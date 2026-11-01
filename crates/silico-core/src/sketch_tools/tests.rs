//! The sketch tools driven by events, as a viewport sends them, against sketches written
//! here. Every expected value is worked by hand from the design (docs/design/m5.0-app.md,
//! section 9, *Sketching*), #278's decisions A to G, the study's *Recommended for M5.11*, and
//! plain geometry (each figure's corners, centres, and radii worked in the test's comment),
//! never from the tools. Each `sketch_add` a tool makes is put through the gate as the
//! command puts it (`sketch_edit::add`), and the next event sees the sketch it leaves, as
//! the newest `sketch` read would show it.

use super::*;
use crate::command::Revision;

mod edges;
use crate::document::json::{self, Plain};
use crate::document::sketch_edit::{self, Members};
use crate::document::{
    Coordinate, Form, Plane, Reference, SketchConstraint, SketchLine, SketchPoint,
};

/// IDs `t0000001`, `t0000002`, ... in turn, skipping any taken.
struct Counter(u32);

impl NewIds for Counter {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        loop {
            self.0 += 1;
            let id = ElementId::new(&format!("t{:07}", self.0)).unwrap();
            if !taken(&id) {
                return Ok(id);
            }
        }
    }
}

const TOLERANCE: f64 = 0.5;

/// The sketch the tools draw in, by its feature's ID.
fn sketch_id() -> FeatureId {
    FeatureId::new("s0000001").unwrap()
}

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
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

fn empty() -> Sketch {
    Sketch {
        plane: Plane::Origin(crate::document::OriginPlane::Xy),
        points: Vec::new(),
        lines: Vec::new(),
        circles: Vec::new(),
        arcs: Vec::new(),
        constraints: Vec::new(),
    }
}

/// A tool driven against a sketch, which takes each `sketch_add` the tool makes.
struct Drive {
    tool: SketchTool,
    sketch: Sketch,
    /// The session's revision: one more for each add the core takes, as a session's.
    revision: Revision,
    tolerance: f64,
    automatic: bool,
    keys: Modifiers,
    ids: Counter,
    /// Each `sketch_add`'s members, described against the sketch it was added to.
    added: Vec<Vec<String>>,
    /// Each add's members, as made.
    made: Vec<Members>,
    messages: Vec<Message>,
    left: bool,
}

impl Drive {
    fn new(kind: ToolKind, sketch: Sketch) -> Self {
        Drive {
            tool: SketchTool::new(kind),
            sketch,
            revision: Revision::OPENED,
            tolerance: TOLERANCE,
            automatic: true,
            keys: Modifiers::default(),
            ids: Counter(0),
            added: Vec::new(),
            made: Vec::new(),
            messages: Vec::new(),
            left: false,
        }
    }

    fn send(&mut self, event: Event) -> Outcome {
        let sketch_id = sketch_id();
        let context = Context {
            sketch_id: &sketch_id,
            revision: self.revision,
            sketch: &self.sketch,
            tolerance: self.tolerance,
            automatic_relations: self.automatic,
        };
        let outcome = self.tool.handle(&context, &mut self.ids, event, self.keys);
        if let Some(members) = &outcome.add {
            self.revision = self.revision.next().unwrap();
            self.tool.added(outcome.token.unwrap(), self.revision);
            let edited = sketch_edit::add(&self.sketch, members)
                .unwrap_or_else(|refused| panic!("the gate refused {members:?}: {refused:?}"));
            let unmet = unmet(&self.sketch, &edited.sketch, members);
            assert!(unmet.is_empty(), "{unmet:?} in {members:?}");
            self.added.push(described(&edited.sketch, members));
            self.made.push(members.clone());
            self.sketch = edited.sketch;
        }
        if let Some(message) = &outcome.message {
            self.messages.push(message.clone());
        }
        self.left |= outcome.left;
        outcome
    }

    fn press(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(Event::Press([x, y]));
        self
    }
    fn to(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(Event::Move([x, y]));
        self
    }
    fn release(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(Event::Release([x, y]));
        self
    }
    fn click(&mut self, x: f64, y: f64) -> &mut Self {
        self.to(x, y).press(x, y).release(x, y)
    }
    fn drag(&mut self, from: [f64; 2], to: [f64; 2]) -> &mut Self {
        self.to(from[0], from[1])
            .press(from[0], from[1])
            .to(to[0], to[1])
            .release(to[0], to[1])
    }
    fn key(&mut self, event: Event) -> &mut Self {
        self.send(event);
        self
    }

    /// The adds so far, each described.
    fn adds(&self) -> Vec<Vec<String>> {
        self.added.clone()
    }
}

/// What of `members`, added to `before` to make `after`, doesn't hold at the coordinates
/// written: each relation, to 1e-9 of the largest coordinate it reads (at least 1 mm), each
/// direction to 1e-9 as a unit vector's; and each new arc's end as far from its centre as its
/// start, at the held points' coordinates as after the add. But for what the tools add
/// without making it hold, by design: a direction a line's end shows where the pointer fixed
/// that end (a point the sketch held, or one placed on a curve or at a middle), which the
/// solve then settles (#278's G).
fn unmet(before: &Sketch, after: &Sketch, members: &Members) -> Vec<String> {
    let held = |id: &ElementId| before.elements().any(|(e, _)| e == id);
    let at = |id: &ElementId| -> Xy {
        let p = after.points.iter().find(|p| &p.id == id).unwrap();
        [p.x.mm(), p.y.mm()]
    };
    let line_of = |r: &Reference| {
        after
            .lines
            .iter()
            .find(|l| Reference::Element(l.id.clone()) == *r)
    };
    // A point the add fixed by a snap: its own `on`, `midpoint`, or the origin's relations.
    let snapped = |id: &ElementId| {
        members.constraints.iter().any(|c| match &c.form {
            Form::On { point, .. } | Form::Midpoint { point, .. } => {
                *point == Reference::Element(id.clone())
            }
            Form::HorizontalPoints { a, b } | Form::VerticalPoints { a, b } => {
                *a == Reference::Element(id.clone()) && *b == Reference::Origin
            }
            _ => false,
        })
    };
    let fixed_end = |r: &Reference| line_of(r).is_none_or(|l| held(&l.to) || snapped(&l.to));
    let place = |r: &Reference| match r {
        Reference::Element(id) => at(id),
        _ => [0.0, 0.0],
    };
    let direction = |r: &Reference| -> Xy {
        match r {
            Reference::XAxis => [1.0, 0.0],
            Reference::YAxis => [0.0, 1.0],
            _ => {
                let l = line_of(r).unwrap();
                geometry::unit(geometry::sub(at(&l.to), at(&l.from))).unwrap()
            }
        }
    };
    let scale = |ps: &[Xy]| {
        ps.iter()
            .flat_map(|p| [p[0].abs(), p[1].abs()])
            .fold(1.0f64, f64::max)
            * 1e-9
    };
    // The direction a curve runs at `point`: a line's own, an arc's or a circle's across its
    // radius there.
    let tangent_of = |r: &Reference, point: Xy| -> Xy {
        if line_of(r).is_some() {
            return direction(r);
        }
        let Reference::Element(id) = r else {
            return direction(r);
        };
        let centre = after
            .arcs
            .iter()
            .find(|a| &a.id == id)
            .map(|a| at(&a.center))
            .unwrap();
        geometry::left(geometry::unit(geometry::sub(point, centre)).unwrap())
    };
    let mut unmet = Vec::new();
    for c in &members.constraints {
        let ok = match &c.form {
            Form::Horizontal { line } => {
                let (a, b) = line_of(line).map(|l| (at(&l.from), at(&l.to))).unwrap();
                fixed_end(line) || (a[1] - b[1]).abs() <= scale(&[a, b])
            }
            Form::Vertical { line } => {
                let (a, b) = line_of(line).map(|l| (at(&l.from), at(&l.to))).unwrap();
                fixed_end(line) || (a[0] - b[0]).abs() <= scale(&[a, b])
            }
            Form::HorizontalPoints { a, b } => {
                let (p, q) = (place(a), place(b));
                (p[1] - q[1]).abs() <= scale(&[p, q])
            }
            Form::VerticalPoints { a, b } => {
                let (p, q) = (place(a), place(b));
                (p[0] - q[0]).abs() <= scale(&[p, q])
            }
            Form::On { point, curve } => {
                let p = place(point);
                let off = match curve {
                    Reference::XAxis => p[1].abs(),
                    Reference::YAxis => p[0].abs(),
                    Reference::Element(id) => {
                        if let Some(l) = line_of(curve) {
                            let (a, b) = (at(&l.from), at(&l.to));
                            geometry::cross(
                                geometry::unit(geometry::sub(b, a)).unwrap(),
                                geometry::sub(p, a),
                            )
                            .abs()
                        } else if let Some(k) = after.circles.iter().find(|k| &k.id == id) {
                            (geometry::distance(p, at(&k.center)) - k.radius.mm()).abs()
                        } else {
                            let a = after.arcs.iter().find(|a| &a.id == id).unwrap();
                            let centre = at(&a.center);
                            (geometry::distance(p, centre)
                                - geometry::distance(at(&a.start), centre))
                            .abs()
                        }
                    }
                    Reference::Origin => f64::INFINITY,
                };
                off <= scale(&[p])
            }
            Form::Midpoint { point, line } => {
                let l = line_of(line).unwrap();
                let mid = geometry::scale(geometry::add(at(&l.from), at(&l.to)), 0.5);
                geometry::distance(place(point), mid) <= scale(&[mid])
            }
            Form::Perpendicular { a, b } => {
                fixed_end(b) || geometry::dot(direction(a), direction(b)).abs() <= 1e-9
            }
            Form::Parallel { a, b } => {
                fixed_end(b) || geometry::cross(direction(a), direction(b)).abs() <= 1e-9
            }
            Form::TangentAt {
                a, b, at: point, ..
            } => {
                let p = place(point);
                (line_of(b).is_some() && fixed_end(b))
                    || geometry::cross(tangent_of(a, p), tangent_of(b, p)).abs() <= 1e-9
            }
            Form::Symmetric { a, b, about } => {
                let mid = geometry::scale(geometry::add(place(a), place(b)), 0.5);
                geometry::distance(mid, place(about)) <= scale(&[mid])
            }
            other => {
                unmet.push(format!("a form no tool adds: {other:?}"));
                true
            }
        };
        if !ok {
            unmet.push(format!("{:?}", c.form));
        }
    }
    // Every arc, its ends held or not: a centrepoint arc's end, the one point not an input
    // to its circle, shares a point only one on the circle (to a picometre).
    for arc in &members.arcs {
        let centre = at(&arc.center);
        let (s, e) = (at(&arc.start), at(&arc.end));
        if (geometry::distance(s, centre) - geometry::distance(e, centre)).abs()
            > scale(&[s, e, centre])
        {
            unmet.push(format!("arc {} isn't round", arc.id));
        }
    }
    unmet
}

/// A coordinate as the descriptions write it: to the nanometre, so a sum's last bit doesn't
/// show.
fn mm(v: f64) -> String {
    let r = (v * 1e9).round() / 1e9;
    format!("{}", if r == 0.0 { 0.0 } else { r })
}

/// `members`, each a line of text, against `sketch` (which holds them): a point by where it
/// is, `(x, y)` if it's one of `members`, `#(x, y)` if the sketch held it before (shared); a
/// line by its ends; a circle by its centre and radius; an arc by its centre, start, and end;
/// each constraint by its type and what it names. Sorted, so the order IDs were drawn in
/// doesn't show.
fn described(sketch: &Sketch, members: &Members) -> Vec<String> {
    let new: std::collections::BTreeSet<&ElementId> =
        members.points.iter().map(|p| &p.id).collect();
    let at = |p: &ElementId| {
        let point = sketch.points.iter().find(|q| &q.id == p).unwrap();
        let mark = if new.contains(p) { "" } else { "#" };
        format!("{mark}({}, {})", mm(point.x.mm()), mm(point.y.mm()))
    };
    let curve = |c: &ElementId| -> String {
        if let Some(l) = sketch.lines.iter().find(|l| &l.id == c) {
            return format!("line {}-{}", at(&l.from), at(&l.to));
        }
        if let Some(k) = sketch.circles.iter().find(|k| &k.id == c) {
            return format!("circle {} r{}", at(&k.center), mm(k.radius.mm()));
        }
        if let Some(a) = sketch.arcs.iter().find(|a| &a.id == c) {
            return format!("arc {} {}->{}", at(&a.center), at(&a.start), at(&a.end));
        }
        at(c)
    };
    let r = |r: &Reference| match r {
        Reference::Element(e) => curve(e),
        other => other.as_str().to_owned(),
    };
    let mut lines: Vec<String> = Vec::new();
    for p in &members.points {
        let kind = if p.construction {
            "construction point"
        } else {
            "point"
        };
        lines.push(format!("{kind} {}", at(&p.id)));
    }
    for l in &members.lines {
        lines.push(curve(&l.id));
    }
    for c in &members.circles {
        lines.push(curve(&c.id));
    }
    for a in &members.arcs {
        lines.push(curve(&a.id));
    }
    for SketchConstraint { form, .. } in &members.constraints {
        lines.push(match form {
            Form::Horizontal { line } => format!("horizontal {}", r(line)),
            Form::Vertical { line } => format!("vertical {}", r(line)),
            Form::HorizontalPoints { a, b } => format!("horizontal_points {} {}", r(a), r(b)),
            Form::VerticalPoints { a, b } => format!("vertical_points {} {}", r(a), r(b)),
            Form::On { point, curve } => format!("on {} {}", r(point), r(curve)),
            Form::Midpoint { point, line } => format!("midpoint {} {}", r(point), r(line)),
            Form::Perpendicular { a, b } => format!("perpendicular {} {}", r(a), r(b)),
            Form::Parallel { a, b } => format!("parallel {} {}", r(a), r(b)),
            Form::Symmetric { a, b, about } => {
                format!("symmetric {} {} about {}", r(a), r(b), r(about))
            }
            Form::TangentAt {
                a,
                b,
                at,
                direction,
            } => format!(
                "tangent_at {} {} at {} {}",
                r(a),
                r(b),
                r(at),
                direction.as_str()
            ),
            other => format!("{other:?}"),
        });
    }
    lines.sort();
    lines
}

fn sorted(lines: &[&str]) -> Vec<String> {
    let mut lines: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
    lines.sort();
    lines
}

// --- The line: SolidWorks' two modes (the M5.11 key test) -----------------------------

#[test]
fn a_click_drag_line_is_one_line_ended_by_its_release() {
    // Press at (10, 10), drag, release at (40, 25): click-drag, one line (§4, *Line*). Its
    // direction, atan(15/30) ≈ 26.6°, is far from horizontal and vertical; its ends line
    // up with nothing.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.drag([10.0, 10.0], [40.0, 25.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (40, 25)",
            "line (10, 10)-(40, 25)"
        ])]
    );
    // The release ended it: the tool's idle, and a move draws no line from the last point.
    assert!(!d.tool.busy());
    d.to(70.0, 70.0);
    assert!(d.tool.preview().lines.is_empty());
    assert!(!d.left);
}

#[test]
fn a_click_click_chain_shares_each_join_and_a_double_click_ends_it() {
    // Clicks at (10, 10), (40, 30), and (60, 5), then a double-click at (60, 5): two
    // segments, the second starting at the first's end, shared (D2). None is within 2° of
    // horizontal or vertical (33.7°, −51.3°), and the second meets the first at 85.0°
    // (cos = (30·20 − 20·25)/(36.06·32.02) = 0.087), 5° from perpendicular.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).click(40.0, 30.0);
    assert!(d.tool.busy(), "the chain goes on");
    d.click(60.0, 5.0);
    // The double-click: its second click lands on the last point (nothing), then the
    // viewport's double-click ends the chain, keeping the tool.
    d.click(60.0, 5.0).key(Event::DoubleClick([60.0, 5.0]));
    assert_eq!(
        d.adds(),
        [
            sorted(&["point (10, 10)", "point (40, 30)", "line (10, 10)-(40, 30)"]),
            sorted(&["point (60, 5)", "line #(40, 30)-(60, 5)"]),
        ]
    );
    // The second segment's start is the first's end, by ID; and the double-click's second
    // click said nothing.
    assert_eq!(d.made[1].lines[0].from, d.made[0].lines[0].to);
    assert!(d.messages.is_empty(), "{:?}", d.messages);
    assert!(!d.tool.busy());
    assert!(!d.left, "a double-click keeps the tool");
    d.to(80.0, 80.0);
    assert!(d.tool.preview().lines.is_empty());
}

#[test]
fn a_chain_clicked_back_on_its_own_start_closes_and_ends() {
    // A triangle: (10, 10), (40, 30), (60, 5), then a click 0.22 mm from the start, within
    // the 0.5 mm tolerance: the third segment ends on the start, shared, and the chain ends
    // there (a click-click line ended on an existing endpoint, §4).
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .click(60.0, 5.0)
        .click(10.2, 10.1);
    let adds = d.adds();
    assert_eq!(adds.len(), 3);
    assert_eq!(adds[2], sorted(&["line #(60, 5)-#(10, 10)"]));
    assert_eq!(d.made[2].lines[0].to, d.made[0].lines[0].from);
    assert!(!d.tool.busy(), "the chain ended at its start");
    // The next click starts afresh: a click alone adds nothing.
    d.click(90.0, 90.0);
    assert_eq!(d.adds().len(), 3);
}

#[test]
fn end_chain_and_enter_end_a_chain_and_keep_the_tool_and_escape_leaves_it() {
    for end in [Event::EndChain, Event::Enter, Event::Escape] {
        let mut d = Drive::new(ToolKind::Line, empty());
        d.click(10.0, 10.0).click(40.0, 30.0).to(55.0, 70.0);
        assert_eq!(d.tool.preview().lines, [[[40.0, 30.0], [55.0, 70.0]]]);
        d.key(end);
        assert!(!d.tool.busy(), "{end:?}");
        assert_eq!(d.left, end == Event::Escape, "{end:?}");
        // The segment clicked stays; the one under way isn't added.
        assert_eq!(d.adds().len(), 1, "{end:?}");
        d.click(70.0, 80.0);
        assert_eq!(d.adds().len(), 1, "{end:?}: a click after it starts afresh");
    }
}

#[test]
fn escape_mid_drag_adds_nothing() {
    let mut d = Drive::new(ToolKind::Line, empty());
    d.to(10.0, 10.0)
        .press(10.0, 10.0)
        .to(30.0, 17.0)
        .key(Event::Escape);
    d.release(30.0, 17.0);
    assert!(d.adds().is_empty());
    assert!(d.left);
}

// --- Automatic relations off, and Ctrl (#278's F) ---------------------------------------

#[test]
fn with_automatic_relations_off_a_chain_shares_its_joins_and_infers_nothing() {
    // (10, 10) to (40, 10.3) is 0.57° from horizontal, and (40, 10.3) to (40.2, 30) 0.58°
    // from vertical: on, each would be so (the next test). Off: the joins are shared all the
    // same, and nothing is inferred.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.automatic = false;
    d.click(10.0, 10.0).click(40.0, 10.3).click(40.2, 30.0);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (10, 10)",
                "point (40, 10.3)",
                "line (10, 10)-(40, 10.3)"
            ]),
            sorted(&["point (40.2, 30)", "line #(40, 10.3)-(40.2, 30)"]),
        ]
    );
}

#[test]
fn with_automatic_relations_on_the_same_chain_is_horizontal_then_vertical() {
    // The same clicks, relations on: the first end moved onto the horizontal through the
    // start, (40, 10); the second onto the vertical through that, (40, 30). Each segment's
    // relation is the pointer's (§2), and the second shows vertical, not perpendicular
    // (vertical is looked for first).
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).click(40.0, 10.3).click(40.2, 30.0);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (10, 10)",
                "point (40, 10)",
                "line (10, 10)-(40, 10)",
                "horizontal line (10, 10)-(40, 10)",
            ]),
            sorted(&[
                "point (40, 30)",
                "line #(40, 10)-(40, 30)",
                "vertical line #(40, 10)-(40, 30)",
            ]),
        ]
    );
}

#[test]
fn with_ctrl_held_a_click_on_a_point_shares_it_and_no_horizontal_is_added() {
    // A lone point at (40, 10). A line from (10, 10) clicked onto it, 0.22 mm off: shared,
    // and the line's exactly horizontal. With Ctrl held, no `horizontal` (an inferred
    // relation), but the point is shared (structure, #278's F), which ends the chain.
    let base = || {
        let mut s = empty();
        s.points.push(point("q0000001", 40.0, 10.0));
        s
    };
    let mut held = Drive::new(ToolKind::Line, base());
    held.keys.ctrl = true;
    held.click(10.0, 10.0).click(40.2, 10.1);
    assert_eq!(
        held.adds(),
        [sorted(&["point (10, 10)", "line (10, 10)-#(40, 10)"])]
    );
    assert_eq!(held.made[0].lines[0].to, id("q0000001"));
    assert!(!held.tool.busy(), "ending on a point ends the chain");
    // Ctrl released, the same clicks: the horizontal is the pointer's.
    let mut free = Drive::new(ToolKind::Line, base());
    free.click(10.0, 10.0).click(40.2, 10.1);
    assert_eq!(
        free.adds(),
        [sorted(&[
            "point (10, 10)",
            "line (10, 10)-#(40, 10)",
            "horizontal line (10, 10)-#(40, 10)",
        ])]
    );
}

#[test]
fn ctrl_held_at_the_accepting_click_alone_decides_its_relations() {
    // Ctrl held for the moves but released for the click: the click is the pointer's, so
    // the horizontal is added. Held only at the click: none.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0);
    d.keys.ctrl = true;
    d.to(40.0, 10.3);
    d.keys.ctrl = false;
    d.press(40.0, 10.3).release(40.0, 10.3);
    assert!(d.adds()[0].contains(&"horizontal line (10, 10)-(40, 10)".to_owned()));
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).to(40.0, 10.3);
    d.keys.ctrl = true;
    d.press(40.0, 10.3).release(40.0, 10.3);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (40, 10.3)",
            "line (10, 10)-(40, 10.3)"
        ])]
    );
}

#[test]
fn a_rectangle_with_relations_off_or_ctrl_keeps_its_shared_corners_and_its_own_relations() {
    // The rectangle's corners are its structure, and its horizontal and vertical its own
    // (FreeCAD 1.1.3's, kept with auto-constraints off; #278's F suppresses only what the
    // pointer infers).
    for (automatic, ctrl) in [(false, false), (true, true)] {
        let mut d = Drive::new(ToolKind::CornerRectangle, empty());
        d.automatic = automatic;
        d.keys.ctrl = ctrl;
        d.drag([10.0, 10.0], [40.0, 30.0]);
        assert_eq!(d.adds(), [rectangle_10_10_40_30()], "{automatic} {ctrl}");
        assert_eq!(d.made[0].points.len(), 4);
    }
}

// --- The corner rectangle -----------------------------------------------------------------

/// The rectangle from (10, 10) to (40, 30): corners in turn (10, 10), (40, 10), (40, 30),
/// (10, 30), the bottom and top horizontal, the sides vertical.
fn rectangle_10_10_40_30() -> Vec<String> {
    sorted(&[
        "point (10, 10)",
        "point (40, 10)",
        "point (40, 30)",
        "point (10, 30)",
        "line (10, 10)-(40, 10)",
        "line (40, 10)-(40, 30)",
        "line (40, 30)-(10, 30)",
        "line (10, 30)-(10, 10)",
        "horizontal line (10, 10)-(40, 10)",
        "vertical line (40, 10)-(40, 30)",
        "horizontal line (40, 30)-(10, 30)",
        "vertical line (10, 30)-(10, 10)",
    ])
}

#[test]
fn a_corner_rectangle_is_four_lines_sharing_four_points_with_its_horizontals_and_verticals() {
    // By a drag, and by two clicks: the same (§4: the first click decides the mode).
    let mut dragged = Drive::new(ToolKind::CornerRectangle, empty());
    dragged.drag([10.0, 10.0], [40.0, 30.0]);
    let mut clicked = Drive::new(ToolKind::CornerRectangle, empty());
    clicked.click(10.0, 10.0).click(40.0, 30.0);
    for d in [&dragged, &clicked] {
        assert_eq!(d.adds(), [rectangle_10_10_40_30()]);
        // Four points, each the end of two of the four lines.
        let members = &d.made[0];
        assert_eq!(members.points.len(), 4);
        for p in &members.points {
            let uses = members
                .lines
                .iter()
                .filter(|l| l.from == p.id || l.to == p.id)
                .count();
            assert_eq!(uses, 2, "{}", p.id);
        }
        assert!(!d.tool.busy());
    }
}

#[test]
fn shift_makes_a_square_of_the_pointers_farther_offset() {
    // From (10, 10) to (40, 25), Shift held: the offset (30, 15), the farther 30, so the
    // square's opposite corner is (40, 40).
    let mut d = Drive::new(ToolKind::CornerRectangle, empty());
    d.keys.shift = true;
    d.drag([10.0, 10.0], [40.0, 25.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (40, 10)",
            "point (40, 40)",
            "point (10, 40)",
            "line (10, 10)-(40, 10)",
            "line (40, 10)-(40, 40)",
            "line (40, 40)-(10, 40)",
            "line (10, 40)-(10, 10)",
            "horizontal line (10, 10)-(40, 10)",
            "vertical line (40, 10)-(40, 40)",
            "horizontal line (40, 40)-(10, 40)",
            "vertical line (10, 40)-(10, 10)",
        ])]
    );
}

#[test]
fn a_rectangle_with_no_height_adds_nothing_and_says_so() {
    let mut d = Drive::new(ToolKind::CornerRectangle, empty());
    d.click(10.0, 10.0).click(40.0, 10.0);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::TooSmall]);
    // Still waiting for its corner: the next click makes it.
    d.click(40.0, 30.0);
    assert_eq!(d.adds(), [rectangle_10_10_40_30()]);
}

// --- #278's A: a point at the origin -------------------------------------------------------

#[test]
fn a_point_clicked_at_the_origin_is_level_with_it_and_above_it() {
    // 0.22 mm from the origin, within 0.5: the point's at (0, 0), with horizontal_points and
    // vertical_points with origin. Ctrl held: where the pointer is, with neither.
    let mut d = Drive::new(ToolKind::Point, empty());
    d.click(0.2, -0.1);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
        ])]
    );
    let shown: Vec<&Relation> = d.tool.preview().shown.iter().map(|s| &s.relation).collect();
    assert_eq!(shown, [&Relation::Origin]);
    let mut held = Drive::new(ToolKind::Point, empty());
    held.keys.ctrl = true;
    held.click(0.2, -0.1);
    assert_eq!(held.adds(), [sorted(&["point (0.2, -0.1)"])]);
}

#[test]
fn a_rectangle_from_the_origin_has_its_corner_there() {
    let mut d = Drive::new(ToolKind::CornerRectangle, empty());
    d.drag([0.3, 0.2], [40.0, 20.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "point (40, 0)",
            "point (40, 20)",
            "point (0, 20)",
            "line (0, 0)-(40, 0)",
            "line (40, 0)-(40, 20)",
            "line (40, 20)-(0, 20)",
            "line (0, 20)-(0, 0)",
            "horizontal line (0, 0)-(40, 0)",
            "vertical line (40, 0)-(40, 20)",
            "horizontal line (40, 20)-(0, 20)",
            "vertical line (0, 20)-(0, 0)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
        ])]
    );
}

#[test]
fn a_point_of_the_sketch_at_the_origin_is_shared_not_placed_there_again() {
    let mut s = empty();
    s.points.push(point("q0000001", 0.0, 0.0));
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([0.1, 0.1], [20.0, 30.0]);
    assert_eq!(
        d.adds(),
        [sorted(&["point (20, 30)", "line #(0, 0)-(20, 30)"])]
    );
}

#[test]
fn the_origin_nearer_than_a_point_wins() {
    // A point at (0.4, 0), and a click at (0.1, 0): 0.3 mm from the point, 0.1 from the
    // origin, both within 0.5. The nearer is the origin (#278's A).
    let mut s = empty();
    s.points.push(point("q0000001", 0.4, 0.0));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(0.1, 0.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
        ])]
    );
    // 0.3 from the origin, 0.1 from the point: the point, so nothing (#278's D).
    d.click(0.3, 0.0);
    assert_eq!(d.messages, [Message::PointThere(id("q0000001"))]);
}

// --- #278's B: tangent arcs only --------------------------------------------------------

/// A line from (10, 10) to (20, 10).
fn one_line() -> Sketch {
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 20.0, 10.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    s
}

#[test]
fn a_tangent_arc_dragged_square_off_its_line_is_tangent_not_normal() {
    // Pressed on the line's end (20, 10), which it leaves along +x, and dragged straight up
    // to (20, 20): SolidWorks' normal zone, but M5 draws tangent arcs only (#278's B). The
    // centre is on the normal (0, 1) at s = |d|²/(2 d·n) = 100/20 = 5: (20, 15), radius 5,
    // turning left, so counter-clockwise from (20, 10) to (20, 20); and `tangent_at`, a
    // smooth join, its own relation.
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.drag([20.1, 10.1], [20.0, 20.0]);
    let tangent =
        "tangent_at line #(10, 10)-#(20, 10) arc (20, 15) #(20, 10)->(20, 20) at #(20, 10) same";
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 15)",
            "point (20, 20)",
            "arc (20, 15) #(20, 10)->(20, 20)",
            tangent,
        ])]
    );
    // Dragged down to (20, 0): s = 100/(2·−10) = −5, the centre (20, 5), turning right:
    // clockwise from (20, 10), so written counter-clockwise from (20, 0) to it.
    let mut down = Drive::new(ToolKind::TangentArc, one_line());
    down.keys.ctrl = true;
    down.drag([20.0, 10.0], [20.0, 0.0]);
    assert_eq!(
        down.adds(),
        [sorted(&[
            "point (20, 5)",
            "point (20, 0)",
            "arc (20, 5) (20, 0)->#(20, 10)",
            "tangent_at line #(10, 10)-#(20, 10) arc (20, 5) (20, 0)->#(20, 10) at #(20, 10) same",
        ])],
        "Ctrl held: the tangency is the tool's own, kept"
    );
}

#[test]
fn a_tangent_arc_follows_an_arc_on_past_its_end() {
    // The arc about (50, 50), radius 10, from (60, 50) to (50, 60): it runs on past its end
    // toward −x. A tangent arc from its end dragged to (40, 70): d = (−10, 10), the normal
    // left of −x is (0, −1), d·n = −10, s = 200/−20 = −10, the centre (50, 60) +
    // (0, −1)(−10) = (50, 70): radius 10, turning right (clockwise), so written from
    // (40, 70) to the end.
    let mut s = empty();
    s.points.push(point("c0000001", 50.0, 50.0));
    s.points.push(point("q0000001", 60.0, 50.0));
    s.points.push(point("q0000002", 50.0, 60.0));
    s.arcs.push(crate::document::SketchArc {
        id: id("a0000001"),
        center: id("c0000001"),
        start: id("q0000001"),
        end: id("q0000002"),
        construction: false,
    });
    let mut d = Drive::new(ToolKind::TangentArc, s);
    d.drag([50.0, 60.0], [40.0, 70.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (50, 70)",
            "point (40, 70)",
            "arc (50, 70) (40, 70)->#(50, 60)",
            "tangent_at arc #(50, 50) #(60, 50)->#(50, 60) arc (50, 70) (40, 70)->#(50, 60) at #(50, 60) same",
        ])]
    );
}

#[test]
fn a_tangent_arc_needs_one_curves_end() {
    // On nothing; on a lone point; on a corner two lines end at: nothing, each said.
    let mut s = one_line();
    s.points.push(point("q0000003", 20.0, 30.0));
    s.lines.push(line("n0000002", "q0000002", "q0000003"));
    s.points.push(point("q0000009", 70.0, 70.0));
    let mut d = Drive::new(ToolKind::TangentArc, s);
    d.drag([0.0, 50.0], [5.0, 60.0]);
    d.drag([70.0, 70.0], [80.0, 80.0]);
    d.drag([20.0, 10.0], [30.0, 15.0]);
    assert!(d.adds().is_empty());
    assert_eq!(
        d.messages,
        [
            Message::NoEndToFollow,
            Message::NoEndToFollow,
            Message::SeveralEnds(id("q0000002")),
        ]
    );
    // The line's free end works.
    d.drag([10.0, 10.0], [10.0, 0.0]);
    assert_eq!(d.adds().len(), 1);
}

#[test]
fn a_tangent_arc_straight_ahead_is_no_arc() {
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.drag([20.0, 10.0], [35.0, 10.0]);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::InLine]);
}

// --- #278's C: the centre rectangle -------------------------------------------------------

#[test]
fn a_centre_rectangle_is_one_construction_point_and_a_symmetric_with_no_diagonals() {
    // Centre (20, 10), corner (30, 15): the opposite corner 2·(20, 10) − (30, 15) = (10, 5);
    // corners in turn (10, 5), (30, 5), (30, 15), (10, 15). One construction point at the
    // centre, `symmetric` of (10, 5) and (30, 15) about it; four lines, no diagonal.
    let mut d = Drive::new(ToolKind::CenterRectangle, empty());
    d.drag([20.0, 10.0], [30.0, 15.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "construction point (20, 10)",
            "point (10, 5)",
            "point (30, 5)",
            "point (30, 15)",
            "point (10, 15)",
            "line (10, 5)-(30, 5)",
            "line (30, 5)-(30, 15)",
            "line (30, 15)-(10, 15)",
            "line (10, 15)-(10, 5)",
            "horizontal line (10, 5)-(30, 5)",
            "vertical line (30, 5)-(30, 15)",
            "horizontal line (30, 15)-(10, 15)",
            "vertical line (10, 15)-(10, 5)",
            "symmetric (10, 5) (30, 15) about (20, 10)",
        ])]
    );
    assert_eq!(d.made[0].lines.len(), 4);
}

// --- #278's D: a point on a point ---------------------------------------------------------

#[test]
fn a_point_clicked_on_a_point_adds_nothing_and_says_so_with_ctrl_too() {
    // A lone point, and a line's end: each clicked on, nothing, and the tool says which.
    let mut s = one_line();
    s.points.push(point("q0000005", 5.0, 5.0));
    for ctrl in [false, true] {
        let mut d = Drive::new(ToolKind::Point, s.clone());
        d.keys.ctrl = ctrl;
        d.click(5.1, 5.2).click(19.8, 10.0);
        assert!(d.adds().is_empty(), "{ctrl}");
        assert_eq!(
            d.messages,
            [
                Message::PointThere(id("q0000005")),
                Message::PointThere(id("q0000002")),
            ],
            "{ctrl}"
        );
        assert!(!d.left, "the tool stays");
    }
}

// --- #278's E: points level or aligned ----------------------------------------------------

#[test]
fn a_point_lined_up_with_another_is_above_it_as_the_pointer_shows() {
    // A point at (30, 10); a click at (30.3, 50), 0.3 mm off its vertical: the pointer shows
    // it above, so the new point is at (30, 50) with `vertical_points`, and a guide from
    // (30, 10) up to it.
    let mut s = empty();
    s.points.push(point("q0000001", 30.0, 10.0));
    let mut d = Drive::new(ToolKind::Point, s.clone());
    // The pointer passes over the point first, waking it (#278's E).
    d.to(30.2, 10.1).to(30.3, 50.0);
    let preview = d.tool.preview();
    assert_eq!(preview.pointer, Some([30.0, 50.0]));
    assert_eq!(preview.guides, [[[30.0, 10.0], [30.0, 50.0]]]);
    let shown: Vec<&Relation> = preview.shown.iter().map(|s| &s.relation).collect();
    assert_eq!(shown, [&Relation::VerticalPoints(id("q0000001"))]);
    d.press(30.3, 50.0).release(30.3, 50.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30, 50)",
            "vertical_points (30, 50) #(30, 10)"
        ])]
    );
    // Ctrl held: neither the move nor the relation.
    let mut held = Drive::new(ToolKind::Point, s);
    held.keys.ctrl = true;
    held.to(30.2, 10.1).click(30.3, 50.0);
    assert_eq!(held.adds(), [sorted(&["point (30.3, 50)"])]);
}

#[test]
fn a_horizontal_lines_end_above_a_point_is_a_guide_not_a_relation() {
    // The help's own example (docs/study/sketch-tool-relations.md, §2, *Inference lines*):
    // a horizontal line whose end is above another point. From (10, 20) to (40.3, 20.2),
    // 0.38° off horizontal: horizontal, the end (40.3, 20); the point (40, 5) is 0.3 mm off
    // its vertical, so the end moves onto it, (40, 20), as a guide, with no
    // `vertical_points` (#278's E: never from a dotted line alone).
    let mut s = empty();
    s.points.push(point("q0000001", 40.0, 5.0));
    let mut d = Drive::new(ToolKind::Line, s);
    d.to(40.1, 5.2).drag([10.0, 20.0], [40.3, 20.2]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 20)",
            "point (40, 20)",
            "line (10, 20)-(40, 20)",
            "horizontal line (10, 20)-(40, 20)",
        ])]
    );
}

// --- #278's G: every relation the pointer shows ----------------------------------------------

#[test]
fn a_redundant_relation_the_pointer_shows_is_added() {
    // From (10, 0.1) to (40, −0.1), each end within 0.5 mm of the x axis: each on it, at
    // (10, 0) and (40, 0), and the line between them horizontal, which the two `on`s already
    // make it. All three are added: no filter (#278's G).
    let mut d = Drive::new(ToolKind::Line, empty());
    d.drag([10.0, 0.1], [40.0, -0.1]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 0)",
            "point (40, 0)",
            "line (10, 0)-(40, 0)",
            "on (10, 0) x_axis",
            "on (40, 0) x_axis",
            "horizontal line (10, 0)-(40, 0)",
        ])]
    );
}

// --- The other relations a line's ends and direction show -------------------------------------

#[test]
fn a_point_takes_a_midpoint_a_crossing_and_a_curve() {
    // Lines (10, 30)-(50, 30) and (33, 10)-(33, 60): the first's middle (30, 30), the
    // second's (33, 35), and their crossing (33, 30). Clicks 0.22 mm from each of the first
    // middle and the crossing, and one 0.2 mm off the first line at x = 45.
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 30.0));
    s.points.push(point("q0000002", 50.0, 30.0));
    s.points.push(point("q0000003", 33.0, 10.0));
    s.points.push(point("q0000004", 33.0, 60.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    s.lines.push(line("n0000002", "q0000003", "q0000004"));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(30.2, 30.1).click(33.1, 29.8).click(45.0, 30.2);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (30, 30)",
                "midpoint (30, 30) line #(10, 30)-#(50, 30)"
            ]),
            sorted(&[
                "point (33, 30)",
                "on (33, 30) line #(10, 30)-#(50, 30)",
                "on (33, 30) line #(33, 10)-#(33, 60)",
            ]),
            sorted(&["point (45, 30)", "on (45, 30) line #(10, 30)-#(50, 30)"]),
        ]
    );
}

#[test]
fn a_line_from_a_lines_end_shows_perpendicular_and_parallel() {
    // The line (10, 10)-(20, 20) leaves its end along (1, 1)/√2. A line from (20, 20) toward
    // (30.1, 10) is 0.29° from perpendicular (its direction (10.1, −10)): moved onto the
    // perpendicular, (20, 20) + (1, −1)/√2 · (10.1·1 + 10·1)/√2 = (30.05, 9.95), and
    // `perpendicular`. Toward (30, 30.2), 0.29° from the line's own direction: parallel,
    // moved to (30.1, 30.1).
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 20.0, 20.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::Line, s.clone());
    d.drag([20.0, 20.0], [30.1, 10.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30.05, 9.95)",
            "line #(20, 20)-(30.05, 9.95)",
            "perpendicular line #(10, 10)-#(20, 20) line #(20, 20)-(30.05, 9.95)",
        ])]
    );
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([20.0, 20.0], [30.0, 30.2]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30.1, 30.1)",
            "line #(20, 20)-(30.1, 30.1)",
            "parallel line #(10, 10)-#(20, 20) line #(20, 20)-(30.1, 30.1)",
        ])]
    );
}

#[test]
fn a_line_leaving_an_arcs_end_smoothly_is_tangent_to_it() {
    // The arc about (50, 50), radius 10, from 0° to 120°: it ends at (45, 50 + 5√3) and
    // runs on along 210° there (120° + 90°), which is neither horizontal nor vertical. A
    // line from its end 20 mm along 211°, 1° off, is tangent (`tangent_at`, same): its end
    // moved onto the 210° ray, 20·cos 1° along it.
    let end = [
        50.0 + 10.0 * libm::cos(120f64.to_radians()),
        50.0 + 10.0 * libm::sin(120f64.to_radians()),
    ];
    let mut s = empty();
    s.points.push(point("c0000001", 50.0, 50.0));
    s.points.push(point("q0000001", 60.0, 50.0));
    s.points.push(point("q0000002", end[0], end[1]));
    s.arcs.push(crate::document::SketchArc {
        id: id("a0000001"),
        center: id("c0000001"),
        start: id("q0000001"),
        end: id("q0000002"),
        construction: false,
    });
    let toward = [
        end[0] + 20.0 * libm::cos(211f64.to_radians()),
        end[1] + 20.0 * libm::sin(211f64.to_radians()),
    ];
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag(end, toward);
    let made = &d.made[0];
    assert_eq!(made.constraints.len(), 1, "{made:?}");
    let line = Reference::Element(made.lines[0].id.clone());
    assert_eq!(
        made.constraints[0].form,
        Form::TangentAt {
            a: Reference::Element(id("a0000001")),
            b: line,
            at: Reference::Element(id("q0000002")),
            direction: crate::document::Direction::Same,
        }
    );
    let p = &made.points[0];
    let along = 20.0 * libm::cos(1f64.to_radians());
    let want = [
        end[0] + along * libm::cos(210f64.to_radians()),
        end[1] + along * libm::sin(210f64.to_radians()),
    ];
    assert!(
        (p.x.mm() - want[0]).abs() < 1e-9 && (p.y.mm() - want[1]).abs() < 1e-9,
        "{p:?} {want:?}"
    );
}

// --- The circle, the arcs, the point -----------------------------------------------------

#[test]
fn a_circle_is_its_centre_then_its_radius_by_a_drag_or_two_clicks() {
    // Centre (10, 10), radius to (13, 14): 5 (a 3-4-5 triangle).
    let want = [sorted(&["point (10, 10)", "circle (10, 10) r5"])];
    let mut dragged = Drive::new(ToolKind::Circle, empty());
    dragged.drag([10.0, 10.0], [13.0, 14.0]);
    assert_eq!(dragged.adds(), want);
    let mut clicked = Drive::new(ToolKind::Circle, empty());
    clicked.click(10.0, 10.0).click(13.0, 14.0);
    assert_eq!(clicked.adds(), want);
}

#[test]
fn a_circles_centre_at_the_origin_and_its_radius_on_a_point() {
    // Centre at the origin (#278's A); the radius set on the point (6, 8): 10, and `on`.
    let mut s = empty();
    s.points.push(point("q0000001", 6.0, 8.0));
    let mut d = Drive::new(ToolKind::Circle, s);
    d.drag([0.2, 0.2], [6.1, 8.2]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
            "circle (0, 0) r10",
            "on #(6, 8) circle (0, 0) r10",
        ])]
    );
}

#[test]
fn a_centrepoint_arc_runs_the_way_the_pointer_swept() {
    // Centre (10, 10), start (18, 16): radius 10, at 36.87°. Swept counter-clockwise through
    // (13, 20) to (4, 18), at 126.87°: from the start to (4, 18). Swept clockwise through
    // (19, 6) to (16, 2), at −53.13°: written counter-clockwise from (16, 2) to the start.
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.drag([10.0, 10.0], [18.0, 16.0])
        .to(13.0, 20.0)
        .click(4.0, 18.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (18, 16)",
            "point (4, 18)",
            "arc (10, 10) (18, 16)->(4, 18)",
        ])]
    );
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.click(10.0, 10.0)
        .click(18.0, 16.0)
        .to(19.0, 6.0)
        .click(16.0, 2.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (18, 16)",
            "point (16, 2)",
            "arc (10, 10) (16, 2)->(18, 16)",
        ])]
    );
}

#[test]
fn a_centrepoint_arcs_preview_runs_the_way_the_pointer_sweeps() {
    // The same arcs, before their last click: counter-clockwise through (13, 20) toward
    // (4, 18), the arc from the start (18, 16) to (4, 18); clockwise through (19, 6) toward
    // (16, 2), the arc from (16, 2) to the start.
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.drag([10.0, 10.0], [18.0, 16.0])
        .to(13.0, 20.0)
        .to(4.0, 18.0);
    assert_eq!(
        d.tool.preview().arcs,
        [PreviewArc {
            center: [10.0, 10.0],
            start: [18.0, 16.0],
            end: [4.0, 18.0],
        }]
    );
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.drag([10.0, 10.0], [18.0, 16.0])
        .to(19.0, 6.0)
        .to(16.0, 2.0);
    assert_eq!(
        d.tool.preview().arcs,
        [PreviewArc {
            center: [10.0, 10.0],
            start: [16.0, 2.0],
            end: [18.0, 16.0],
        }]
    );
}

#[test]
fn a_centrepoint_arcs_end_is_on_the_circle_its_start_sets() {
    // The end clicked at (−2, 26), 20 from the centre toward (−0.6, 0.8): on the circle of
    // radius 10, at (4, 18).
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.drag([10.0, 10.0], [18.0, 16.0])
        .to(13.0, 20.0)
        .click(-2.0, 26.0);
    assert!(d.adds()[0].contains(&"arc (10, 10) (18, 16)->(4, 18)".to_owned()));
}

#[test]
fn a_three_point_arc_passes_through_its_third_click() {
    // Start (10, 10), end (30, 10), through (20, 20): the circle about (20, 10) of radius 10;
    // (20, 20) is left of the chord from the start, so the arc runs counter-clockwise from
    // the end (30, 10) over the top to the start.
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.click(10.0, 10.0).click(30.0, 10.0).click(20.0, 20.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 10)",
            "point (10, 10)",
            "point (30, 10)",
            "arc (20, 10) (30, 10)->(10, 10)",
        ])]
    );
    // Through (20, 0), below: counter-clockwise from the start.
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.drag([10.0, 10.0], [30.0, 10.0]).click(20.0, 0.0);
    assert!(d.adds()[0].contains(&"arc (20, 10) (10, 10)->(30, 10)".to_owned()));
}

#[test]
fn a_three_point_arc_through_a_point_in_line_adds_nothing_and_waits() {
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.click(10.0, 10.0).click(30.0, 20.0).click(50.0, 30.0);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::InLine]);
    d.click(20.0, 25.0);
    assert_eq!(d.adds().len(), 1);
}

#[test]
fn the_point_tool_stays_and_places_a_point_at_each_click() {
    let mut d = Drive::new(ToolKind::Point, empty());
    d.click(12.0, 34.0).click(56.0, 78.0);
    assert_eq!(
        d.adds(),
        [sorted(&["point (12, 34)"]), sorted(&["point (56, 78)"])]
    );
    assert!(!d.left);
}

#[test]
fn escape_leaves_each_tool_mid_gesture_adding_nothing() {
    for kind in ToolKind::ALL {
        let mut d = Drive::new(kind, one_line());
        // A first press and release (on the line's free end, for the tangent arc), then a
        // move: each tool's gesture is under way.
        d.click(10.0, 10.0).to(37.0, 41.0);
        if kind != ToolKind::Point {
            assert!(d.tool.busy(), "{kind:?}");
        }
        d.key(Event::Escape);
        assert!(d.left, "{kind:?}");
        assert!(!d.tool.busy(), "{kind:?}");
        assert!(d.adds().is_empty(), "{kind:?}");
    }
}

// --- What the pointer shows ---------------------------------------------------------------

#[test]
fn the_preview_shows_what_the_click_would_add() {
    // Hovering 0.1 mm off the x axis mid-line: the line under way to (40, 0), `on` x_axis and
    // horizontal; the same as what the release adds (the G test).
    let mut d = Drive::new(ToolKind::Line, empty());
    d.press(10.0, 0.1).to(40.0, -0.1);
    let preview = d.tool.preview();
    assert_eq!(preview.lines, [[[10.0, 0.0], [40.0, 0.0]]]);
    let shown: Vec<&Relation> = preview.shown.iter().map(|s| &s.relation).collect();
    assert_eq!(
        shown,
        [
            &Relation::On(Reference::XAxis),
            &Relation::On(Reference::XAxis),
            &Relation::Horizontal,
        ]
    );
}

// --- Hostile input ------------------------------------------------------------------------

#[test]
fn a_position_that_isnt_a_number_is_refused_and_changes_nothing() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut d = Drive::new(ToolKind::Line, empty());
        d.click(10.0, 10.0);
        let outcome = d.send(Event::Press([bad, 5.0]));
        assert_eq!(outcome.message, Some(Message::NotFinite));
        d.send(Event::Release([5.0, bad]));
        d.send(Event::Move([bad, bad]));
        // The chain's still there, from (10, 10).
        d.click(40.0, 30.0);
        assert_eq!(
            d.adds(),
            [sorted(&[
                "point (10, 10)",
                "point (40, 30)",
                "line (10, 10)-(40, 30)"
            ])]
        );
    }
}

#[test]
fn a_point_past_a_kilometre_adds_nothing_and_says_so() {
    let mut d = Drive::new(ToolKind::Point, empty());
    d.click(1e6, 7.0);
    assert_eq!(
        d.adds(),
        [sorted(&["point (1000000, 7)"])],
        "a kilometre is within reach"
    );
    d.click(1e6 + 1.0, 0.0).click(0.0, -2e300);
    assert_eq!(d.adds().len(), 1);
    assert_eq!(d.messages, [Message::TooFar, Message::TooFar]);
    let mut d = Drive::new(ToolKind::Line, empty());
    d.drag([10.0, 10.0], [10.0, 2e6]);
    assert_eq!(d.messages, [Message::TooFar]);
    let mut d = Drive::new(ToolKind::Circle, empty());
    d.drag([10.0, 10.0], [10.0, 1e6 + 20.0]);
    assert_eq!(d.messages, [Message::TooFar]);
    assert!(d.adds().is_empty());
}

#[test]
fn a_tolerance_that_isnt_a_positive_number_snaps_to_nothing() {
    // A point at (5, 5) and a click exactly on it: shared with any tolerance (0 included, the
    // pointer is no distance off). A click at (5.1, 5.3), 0.32 mm off and lined up with
    // nothing: with NaN, −1, or ∞ as the tolerance, it's taken as 0, and the click's a new
    // point.
    let mut s = empty();
    s.points.push(point("q0000001", 5.0, 5.0));
    for bad in [f64::NAN, -1.0, f64::INFINITY, 0.0] {
        let mut d = Drive::new(ToolKind::Point, s.clone());
        d.tolerance = bad;
        d.click(5.0, 5.0).click(5.1, 5.3);
        assert_eq!(d.messages, [Message::PointThere(id("q0000001"))], "{bad}");
        assert_eq!(d.adds(), [sorted(&["point (5.1, 5.3)"])], "{bad}");
    }
}

#[test]
fn the_tolerance_is_inclusive() {
    // A point at (5, 5): a click 0.5 mm off, exactly the tolerance (0.5 is exact in binary,
    // and 5.5 − 5 is too), is on it; the next float past isn't.
    let mut s = empty();
    s.points.push(point("q0000001", 5.0, 5.0));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(5.5, 5.0);
    assert_eq!(d.messages, [Message::PointThere(id("q0000001"))]);
    d.click(f64::from_bits(5.5f64.to_bits() + 1), 5.0);
    assert_eq!(d.adds().len(), 1);
}

#[test]
fn coincident_clicks_and_zero_drags_draw_nothing_degenerate() {
    // A press and release in one place is a click, not a line of no length; a second click
    // there is a double-click's, ignored; a release with no press is ignored.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).click(10.0, 10.0).click(10.3, 10.0);
    d.send(Event::Release([50.0, 50.0]));
    assert!(d.adds().is_empty());
    assert!(d.messages.is_empty());
    // The circle's, a rectangle's, and the arcs' second click on their first: nothing.
    for kind in [
        ToolKind::Circle,
        ToolKind::CornerRectangle,
        ToolKind::CenterRectangle,
        ToolKind::CenterpointArc,
        ToolKind::ThreePointArc,
    ] {
        let mut d = Drive::new(kind, empty());
        d.click(10.0, 10.0).click(10.2, 10.0);
        assert!(d.adds().is_empty(), "{kind:?}");
        assert!(d.tool.busy(), "{kind:?}: still waiting");
    }
    // A line dragged from a point back onto it: too small.
    let mut s = empty();
    s.points.push(point("q0000001", 5.0, 5.0));
    let mut d = Drive::new(ToolKind::Line, s);
    d.to(5.0, 5.0)
        .press(5.0, 5.0)
        .to(20.0, 20.0)
        .release(5.1, 5.1);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::TooSmall]);
}

#[test]
fn two_points_a_hair_apart_draw_no_line_the_kernel_cant() {
    // Two points of the sketch 1e-8 mm apart: a line between them is under the kernel's
    // 1e-7 mm, so it's refused here, as the gate would.
    let mut s = empty();
    s.points.push(point("q0000001", 5.0, 5.0));
    s.points.push(point("q0000002", 5.0 + 1e-8, 5.0));
    let mut d = Drive::new(ToolKind::Line, s);
    d.tolerance = 1e-9;
    d.click(5.0, 5.0).click(5.0 + 1e-8, 5.0);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::TooSmall]);
}

#[test]
fn the_members_are_what_new_members_writes() {
    // `NewMembers::of`, read back as a request's lists: the point and the line, each list
    // sorted by ID, every field written.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.drag([10.0, 10.0], [40.0, 25.5]);
    let new = crate::command::NewMembers::of(&d.made[0]).unwrap();
    let plain = |text: &str| Plain::of(&json::read(text.as_bytes()).unwrap());
    assert_eq!(
        Plain::of(&new.points),
        plain(
            r#"[{"id": "t0000001", "x": 10, "y": 10, "construction": false}, {"id": "t0000002", "x": 40, "y": 25.5, "construction": false}]"#
        )
    );
    assert_eq!(
        Plain::of(&new.lines),
        plain(
            r#"[{"id": "t0000003", "from": "t0000001", "to": "t0000002", "construction": false}]"#
        )
    );
    for empty in [&new.circles, &new.arcs, &new.constraints] {
        assert_eq!(Plain::of(empty), plain("[]"));
    }
}

#[test]
fn a_centrepoint_arcs_end_snapped_off_its_circle_is_put_back_on_it() {
    // Found by the property below: with a pick tolerance past the radius, the end's place on
    // the circle snapped to the centre, a point of the sketch, off the circle. Centre the
    // point (20, 10), start the point (0, 0), radius √500; the end toward (25, 12) is
    // (20, 10) + √500·(5, 2)/√29, which a 30 mm tolerance would snap to the centre. The end
    // stays on the circle, a new point, with no relation.
    let mut s = empty();
    s.points.push(point("q0000001", 20.0, 10.0));
    s.points.push(point("q0000002", 0.0, 0.0));
    let mut d = Drive::new(ToolKind::CenterpointArc, s);
    d.tolerance = 30.0;
    d.click(20.0, 10.0)
        .click(0.0, 0.0)
        .to(-5.0, 30.0)
        .click(25.0, 12.0);
    assert!(d.messages.is_empty(), "{:?}", d.messages);
    let made = &d.made[0];
    assert_eq!(made.points.len(), 1);
    assert!(made.constraints.is_empty());
    let end = [made.points[0].x.mm(), made.points[0].y.mm()];
    let scale = 500f64.sqrt() / 29f64.sqrt();
    assert!((end[0] - (20.0 + 5.0 * scale)).abs() < 1e-9, "{end:?}");
    assert!((end[1] - (10.0 + 2.0 * scale)).abs() < 1e-9, "{end:?}");
    assert_eq!(made.arcs[0].center, id("q0000001"));
}

#[test]
fn an_arcs_radius_at_a_kilometre_is_measured_as_the_gate_measures_it() {
    // Found by the property below: a centrepoint arc of radius 1e6 mm, swept clockwise, is
    // written from its end, placed on the circle by the pointer's direction; that point's
    // distance from the centre, as the gate measures it (the square root of the squares'
    // sum), can be a rounding past 1e6, which the gate refuses. Centre at the origin, start
    // (1e6, 0) on the x axis: find a direction whose point on the circle is past 1e6 that
    // way, and sweep to it. The arc is refused as too far, never sent for the gate to refuse.
    let radius = 1e6;
    let past = (1..2000)
        .map(|k| -f64::from(k) * 0.0007)
        .map(|angle| [libm::cos(angle), libm::sin(angle)])
        .find(|direction| {
            let length = libm::hypot(direction[0], direction[1]);
            let p = [
                direction[0] / length * radius,
                direction[1] / length * radius,
            ];
            (p[0] * p[0] + p[1] * p[1]).sqrt() > radius
        })
        .expect("a direction whose point is a rounding past the radius");
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.click(0.1, 0.1).click(1e6, 0.2);
    let toward = [past[0] * 5e5, past[1] * 5e5];
    d.to(toward[0], toward[1]).click(toward[0], toward[1]);
    assert!(d.adds().is_empty(), "{:?}", d.adds());
    assert_eq!(d.messages, [Message::TooFar]);
}

/// `events`, each a click at a place, sent to `tool` against `sketch`, which no add changes:
/// the newest read, before any add reaches it. What each click added, or said.
fn clicks_on_a_stale_read(tool: &mut SketchTool, sketch: &Sketch, at: &[[f64; 2]]) -> Vec<Outcome> {
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut ids = Counter(0);
    let keys = Modifiers::default();
    at.iter()
        .map(|&p| {
            tool.handle(&context, &mut ids, Event::Press(p), keys);
            tool.handle(&context, &mut ids, Event::Release(p), keys)
        })
        .collect()
}

#[test]
fn what_a_tool_added_is_found_before_a_read_shows_it() {
    // The read never catches up. A point clicked twice in one place: the second click finds
    // the first point, t0000001, and adds nothing (#278's D).
    let sketch = empty();
    let mut tool = SketchTool::new(ToolKind::Point);
    let out = clicks_on_a_stale_read(&mut tool, &sketch, &[[12.0, 34.0], [12.1, 34.0]]);
    assert!(out[0].add.is_some());
    assert_eq!(out[1].add, None);
    assert_eq!(out[1].message, Some(Message::PointThere(id("t0000001"))));
    // A triangle: each segment shares the last one's end, and the last click, on the start,
    // shares it and ends the chain, though no read holds any of it.
    let mut tool = SketchTool::new(ToolKind::Line);
    let out = clicks_on_a_stale_read(
        &mut tool,
        &sketch,
        &[[10.0, 10.0], [40.0, 30.0], [60.0, 5.0], [10.2, 10.1]],
    );
    let lines: Vec<&SketchLine> = out
        .iter()
        .filter_map(|o| o.add.as_ref())
        .flat_map(|m| &m.lines)
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1].from, lines[0].to);
    assert_eq!(lines[2].from, lines[1].to);
    assert_eq!(lines[2].to, lines[0].from);
    assert!(!tool.busy());
}

#[test]
fn what_the_tool_forgets_is_not_found_again() {
    // An add the core refused: forgotten, the next click in its place adds a point anew.
    let sketch = empty();
    let mut tool = SketchTool::new(ToolKind::Point);
    clicks_on_a_stale_read(&mut tool, &sketch, &[[12.0, 34.0]]);
    tool.forget_pending();
    let out = clicks_on_a_stale_read(&mut tool, &sketch, &[[12.0, 34.0]]);
    assert!(out[0].add.is_some(), "{out:?}");
    // Once a read holds what was added, it's the read's: the tool drops its own copy, so a
    // read without it (an undo) finds nothing there.
    let mut tool = SketchTool::new(ToolKind::Point);
    let out = clicks_on_a_stale_read(&mut tool, &sketch, &[[12.0, 34.0]]);
    let added = out[0].add.clone().unwrap();
    let held = sketch_edit::add(&sketch, &added).unwrap().sketch;
    let out = clicks_on_a_stale_read(&mut tool, &held, &[[12.0, 34.0]]);
    assert_eq!(out[0].message, Some(Message::PointThere(id("t0000001"))));
    let out = clicks_on_a_stale_read(&mut tool, &sketch, &[[12.0, 34.0]]);
    assert!(out[0].add.is_some(), "{out:?}");
}

/// An ID source that has none to give.
struct Dry;

impl NewIds for Dry {
    fn next(&mut self, _: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        Err(Error::new(ErrorCode::IoFailed, "no randomness"))
    }
}

#[test]
fn a_source_with_no_ids_adds_nothing_and_says_why() {
    let sketch = empty();
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut tool = SketchTool::new(ToolKind::Line);
    let keys = Modifiers::default();
    for event in [Event::Press([10.0, 10.0]), Event::Move([40.0, 30.0])] {
        assert_eq!(
            tool.handle(&context, &mut Dry, event, keys),
            Outcome::default()
        );
    }
    let outcome = tool.handle(&context, &mut Dry, Event::Release([40.0, 30.0]), keys);
    assert_eq!(outcome.add, None);
    assert_eq!(
        outcome.message,
        Some(Message::NoId(Error::new(
            ErrorCode::IoFailed,
            "no randomness"
        )))
    );
    assert_eq!(
        outcome.message.map(|m| m.text()).as_deref(),
        Some("no new ID could be made: no randomness")
    );
}

#[test]
fn the_systems_ids_are_fresh_and_none_taken() {
    let mut ids = SystemIds::new();
    let mut drawn: Vec<ElementId> = Vec::new();
    for _ in 0..64 {
        let id = ids.next(&|id| drawn.contains(id)).unwrap();
        assert!(!drawn.contains(&id));
        drawn.push(id);
    }
}

// --- Any events at all ---------------------------------------------------------------------

mod property {
    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;

    use super::*;

    /// A sketch with something of every kind to snap to: a line, an arc, a circle, and a lone
    /// point, a point at the origin, and two points a hair (1e-8 mm) apart.
    fn busy() -> Sketch {
        let mut s = one_line();
        s.points.push(point("c0000001", 50.0, 50.0));
        s.points.push(point("q0000003", 60.0, 50.0));
        s.points.push(point("q0000004", 50.0, 60.0));
        s.arcs.push(crate::document::SketchArc {
            id: id("a0000001"),
            center: id("c0000001"),
            start: id("q0000003"),
            end: id("q0000004"),
            construction: false,
        });
        s.points.push(point("c0000002", -20.0, 30.0));
        s.circles.push(crate::document::SketchCircle {
            id: id("k0000001"),
            center: id("c0000002"),
            radius: Coordinate::new(7.0).unwrap(),
            construction: false,
        });
        s.points.push(point("q0000005", 0.0, 0.0));
        s.points.push(point("q0000006", 30.0, -30.0));
        s.points.push(point("q0000007", 30.0 + 1e-8, -30.0));
        s
    }

    /// A coordinate: mostly near the sketch's members, some at its edges, some not numbers.
    fn coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![
            8 => -70.0..70.0f64,
            2 => prop::sample::select(vec![0.0, 10.0, 20.0, 30.0, 50.0, 60.0, -20.0, -30.0]),
            1 => prop::sample::select(vec![
                30.0 + 1e-8,
                30.0 + 5e-9,
                1e6,
                -1e6,
                1e6 + 1.0,
                1e300,
                f64::NAN,
                f64::INFINITY,
                f64::MIN_POSITIVE,
            ]),
        ]
    }

    fn event() -> impl Strategy<Value = (Event, Modifiers)> {
        let at = || (coordinate(), coordinate()).prop_map(|(x, y)| [x, y]);
        let event = prop_oneof![
            4 => at().prop_map(Event::Press),
            4 => at().prop_map(Event::Move),
            4 => at().prop_map(Event::Release),
            1 => at().prop_map(Event::DoubleClick),
            1 => Just(Event::EndChain),
            1 => Just(Event::Escape),
            1 => Just(Event::Enter),
        ];
        (event, any::<bool>(), any::<bool>())
            .prop_map(|(event, shift, ctrl)| (event, Modifiers { shift, ctrl }))
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 600,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(0x5c_7001),
            ..ProptestConfig::default()
        })]

        /// Whatever the events, the tolerance, and the setting: nothing panics, and every
        /// `sketch_add` a tool makes is one the gate takes (`Drive::send` puts each through
        /// it, the next event against the sketch it leaves), writes as a request's lists, and
        /// places each new point finite and within a kilometre.
        #[test]
        fn any_events_make_only_adds_the_gate_takes(
            kind in prop::sample::select(ToolKind::ALL.to_vec()),
            events in prop::collection::vec(event(), 0..48),
            tolerance in prop::sample::select(vec![0.0, 1e-9, 0.5, 5.0, 1e3, f64::NAN, -1.0]),
            automatic in any::<bool>(),
        ) {
            let mut d = Drive::new(kind, busy());
            d.tolerance = tolerance;
            d.automatic = automatic;
            for (event, keys) in events {
                d.keys = keys;
                d.send(event);
            }
            for members in &d.made {
                prop_assert!(crate::command::NewMembers::of(members).is_ok());
                for p in &members.points {
                    prop_assert!(within_reach([p.x.mm(), p.y.mm()]), "{p:?}");
                }
                for c in &members.circles {
                    prop_assert!(c.radius.mm() > 0.0 && c.radius.mm() <= 1e6, "{c:?}");
                }
            }
        }
    }
}
