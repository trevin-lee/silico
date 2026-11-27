//! Sketch mode's tools driven as the viewport drives them: a frame's left-button events at
//! screen points, through 11a's hand projection (ten points a millimetre, the origin at
//! (100, 500), y up, so the 7-point pick is 0.7 mm), into the core's tools and trim and
//! split; each `sketch_add` put through the gate as the command puts it
//! (`sketch_edit::add`), answered by its token, and read back as the newest `sketch` read.
//! Every expected member, place, and relation is worked by hand from the clicks and the
//! design (docs/design/m5.0-app.md, section 9, *Sketching*), never from the tools.

use eframe::egui::Pos2;
use silico_core::command::Revision;
use silico_core::document::sketch_edit::{self, Members};
use silico_core::document::{
    ElementId, FeatureId, Form, OriginPlane, Reference, Sketch, SketchCircle,
};
use silico_core::extend::End;
use silico_core::sketch_tools::{AddToken, Event, ToolKind};

use super::*;
use crate::sketch::tests::{hand, id, line, point, under, unhand, with_painter};
use crate::sketch::{FrameInput, PointerAction, PointerEvent, Read, SketchMode};

/// The screen point of the sketch's (x, y).
fn at(x: f64, y: f64) -> Pos2 {
    hand([x, y]).unwrap()
}

fn ev(action: PointerAction, x: f64, y: f64) -> PointerEvent {
    PointerEvent {
        action,
        at: at(x, y),
        shift: false,
        ctrl: false,
    }
}

fn click(x: f64, y: f64) -> Vec<PointerEvent> {
    vec![
        ev(PointerAction::Press, x, y),
        ev(PointerAction::Release, x, y),
    ]
}

/// A press at `from`, a move halfway, and the release at `to`.
fn drag(from: [f64; 2], to: [f64; 2]) -> Vec<PointerEvent> {
    let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    vec![
        ev(PointerAction::Press, from[0], from[1]),
        ev(PointerAction::Move, mid[0], mid[1]),
        ev(PointerAction::Move, to[0], to[1]),
        ev(PointerAction::Release, to[0], to[1]),
    ]
}

fn ctrl(mut events: Vec<PointerEvent>) -> Vec<PointerEvent> {
    for event in &mut events {
        event.ctrl = true;
    }
    events
}

fn shift(mut events: Vec<PointerEvent>) -> Vec<PointerEvent> {
    for event in &mut events {
        event.shift = true;
    }
    events
}

fn empty() -> Sketch {
    Sketch {
        plane: OriginPlane::Xy.into(),
        points: Vec::new(),
        lines: Vec::new(),
        circles: Vec::new(),
        arcs: Vec::new(),
        constraints: Vec::new(),
    }
}

fn sketch_id() -> FeatureId {
    FeatureId::new("s0000001").unwrap()
}

/// Sketch mode on a sketch, the core stood in for: each add the gate's, answered at once
/// (unless `holding`, when it waits in `held` for the test to answer), the session's
/// revision one on, and the sketch read again at it.
struct Desk {
    mode: SketchMode,
    sketch: Sketch,
    revision: Revision,
    holding: bool,
    held: Vec<(AddToken, Members)>,
    adds: Vec<Members>,
    edits: Vec<Request>,
}

impl Desk {
    fn new(choice: Choice, sketch: Sketch) -> Self {
        let mut mode = SketchMode::new(1, Some(sketch_id()));
        mode.tools.choose(Some(choice));
        let mut desk = Desk {
            mode,
            sketch,
            revision: Revision::OPENED,
            holding: false,
            held: Vec::new(),
            adds: Vec::new(),
            edits: Vec::new(),
        };
        desk.publish();
        desk
    }

    /// The sketch read at the session's revision.
    fn publish(&mut self) {
        self.mode.took(Read {
            model: Some(self.revision),
            result: Ok(under(self.sketch.clone())),
        });
    }

    /// One frame with `events`, the pointer at the last of them.
    fn frame(&mut self, events: &[PointerEvent]) {
        let pointer = events.last().map(|event| event.at);
        self.frame_at(events, pointer);
    }

    fn frame_at(&mut self, events: &[PointerEvent], pointer: Option<Pos2>) {
        let mode = &mut self.mode;
        with_painter(|painter| {
            let input = FrameInput {
                events,
                pointer,
                project: &hand,
                to_plane: &unhand,
            };
            mode.frame(painter, &input, &mut None);
        });
        self.take();
    }

    /// What the tools asked: each add the gate takes, answered (or held), the rest kept.
    fn take(&mut self) {
        for request in self.mode.tools.take_requests() {
            match request {
                Request::Add { token, members } if self.holding => {
                    self.held.push((token, members));
                }
                Request::Add { token, members } => {
                    let revision = self.apply(&members);
                    self.mode.tools.added(token, revision);
                    self.publish();
                }
                other => self.edits.push(other),
            }
        }
    }

    /// `members` added, as `sketch_add` adds them: the revision it makes.
    fn apply(&mut self, members: &Members) -> Revision {
        let edited = sketch_edit::add(&self.sketch, members)
            .unwrap_or_else(|refused| panic!("the gate refused {members:?}: {refused:?}"));
        self.sketch = edited.sketch;
        self.revision = self.revision.next().unwrap();
        self.adds.push(members.clone());
        self.revision
    }

    /// The place of the point `id`, in the sketch.
    fn place(&self, id: &ElementId) -> [f64; 2] {
        let p = self.sketch.points.iter().find(|p| &p.id == id).unwrap();
        [p.x.mm(), p.y.mm()]
    }

    fn said(&self) -> Option<&str> {
        self.mode.tools.said()
    }
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

/// Each line of `members` as the places of its two ends, sorted, in the desk's sketch.
fn sides(desk: &Desk, members: &Members) -> Vec<[[f64; 2]; 2]> {
    let mut sides: Vec<[[f64; 2]; 2]> = members
        .lines
        .iter()
        .map(|l| {
            let mut ends = [desk.place(&l.from), desk.place(&l.to)];
            ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
            ends
        })
        .collect();
    sides.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sides
}

/// The lines of `members` that a `horizontal` (or, `vertical`) names, as [`sides`] gives
/// them.
fn held(desk: &Desk, members: &Members, horizontal: bool) -> Vec<[[f64; 2]; 2]> {
    let named: Vec<&ElementId> = members
        .constraints
        .iter()
        .filter_map(|c| match (&c.form, horizontal) {
            (
                Form::Horizontal {
                    line: Reference::Element(l),
                },
                true,
            )
            | (
                Form::Vertical {
                    line: Reference::Element(l),
                },
                false,
            ) => Some(l),
            _ => None,
        })
        .collect();
    let lines = Members {
        lines: members
            .lines
            .iter()
            .filter(|l| named.contains(&&l.id))
            .cloned()
            .collect(),
        ..Members::default()
    };
    sides(desk, &lines)
}

/// The places of `members`' points, sorted.
fn places(desk: &Desk, members: &Members) -> Vec<[f64; 2]> {
    let mut places: Vec<[f64; 2]> = members.points.iter().map(|p| desk.place(&p.id)).collect();
    places.sort_by(|a, b| a.partial_cmp(b).unwrap());
    places
}

/// A 40 × 20 rectangle from (10, 10) to (50, 30): its corners, its sides, and which are
/// horizontal and vertical, by hand.
fn assert_rectangle(desk: &Desk, members: &Members) {
    let (a, b, c, d) = ([10.0, 10.0], [10.0, 30.0], [50.0, 10.0], [50.0, 30.0]);
    assert_eq!(places(desk, members), [a, b, c, d]);
    assert_eq!(members.points.len(), 4, "four points, each corner's");
    assert_eq!(sides(desk, members), [[a, b], [a, c], [b, d], [c, d]]);
    // Four lines on four shared points: each corner ends exactly two sides.
    for p in &members.points {
        let ends = members
            .lines
            .iter()
            .filter(|l| l.from == p.id || l.to == p.id)
            .count();
        assert_eq!(ends, 2, "{}", p.id);
    }
    assert_eq!(held(desk, members, true), [[a, c], [b, d]]);
    assert_eq!(held(desk, members, false), [[a, b], [c, d]]);
    assert_eq!(members.constraints.len(), 4, "{:?}", members.constraints);
}

#[test]
fn a_corner_rectangle_dragged_makes_four_lines_on_four_shared_points_two_horizontal_two_vertical() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.frame(&drag([10.0, 10.0], [50.0, 30.0]));
    assert_eq!(desk.adds.len(), 1);
    let members = desk.adds[0].clone();
    assert_rectangle(&desk, &members);
    // The tool stays, nothing under way.
    assert_eq!(
        desk.mode.tools.choice(),
        Some(Choice::Draw(ToolKind::CornerRectangle))
    );
    assert!(!desk.mode.tools.busy());
}

#[test]
fn a_corner_rectangle_by_two_clicks_makes_the_same() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.frame(&click(10.0, 10.0));
    assert!(desk.adds.is_empty());
    assert!(desk.mode.tools.busy());
    // The pointer moved, no button: the preview follows it, a rectangle dashed to it.
    desk.frame_at(&[], Some(at(30.0, 20.0)));
    let preview = desk.mode.tools.preview().clone();
    assert_eq!(preview.pointer, Some([30.0, 20.0]));
    assert_eq!(preview.lines.len(), 4, "{preview:?}");
    desk.frame(&click(50.0, 30.0));
    assert_eq!(desk.adds.len(), 1);
    let members = desk.adds[0].clone();
    assert_rectangle(&desk, &members);
}

#[test]
fn a_line_chain_shares_its_points_and_ends_by_double_click_enter_end_chain_and_esc() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    // Clicks at (10, 10), (30, 15), and (30, 40): two segments, the second from the
    // first's end, which they share.
    for [x, y] in [[10.0, 10.0], [30.0, 15.0], [30.0, 40.0]] {
        desk.frame(&click(x, y));
    }
    assert_eq!(desk.adds.len(), 2);
    let (first, second) = (&desk.adds[0].lines[0], &desk.adds[1].lines[0]);
    assert_eq!(second.from, first.to);
    assert!(near(desk.place(&first.from), [10.0, 10.0]));
    assert!(near(desk.place(&first.to), [30.0, 15.0]));
    assert!(near(desk.place(&second.to), [30.0, 40.0]));
    // The second shared its start: it added one point, its end.
    assert_eq!(desk.adds[1].points.len(), 1);
    // (30, 15) to (30, 40) is vertical, and shows it: `vertical`.
    assert!(matches!(
        desk.adds[1].constraints.as_slice(),
        [c] if c.form == Form::Vertical { line: Reference::Element(second.id.clone()) }
    ));
    assert!(desk.mode.tools.busy());

    // A double-click (the viewport's press, release, press, release, then the double-click)
    // at (50, 40): one more segment, the second click adding nothing, then the chain ended,
    // the tool kept.
    let mut double = click(50.0, 40.0);
    double.extend(click(50.0, 40.0));
    double.push(ev(PointerAction::DoubleClick, 50.0, 40.0));
    desk.frame(&double);
    assert_eq!(desk.adds.len(), 3);
    assert!(!desk.mode.tools.busy());
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    // The next click starts a chain afresh: no segment from (50, 40).
    desk.frame(&click(60.0, 60.0));
    assert_eq!(desk.adds.len(), 3);
    assert!(desk.mode.tools.busy());

    // Enter ends it, the tool kept.
    desk.mode.end_chain(Event::Enter);
    assert!(!desk.mode.tools.busy());
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    // End chain, from the canvas menu, likewise.
    desk.frame(&click(60.0, 60.0));
    assert!(desk.mode.tools.busy());
    desk.mode.end_chain(Event::EndChain);
    assert!(!desk.mode.tools.busy());
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    // Esc ends it and leaves the tool.
    desk.frame(&click(60.0, 60.0));
    assert!(desk.mode.escape_tool());
    assert_eq!(desk.mode.tools.choice(), None);
    assert!(!desk.mode.escape_tool(), "no tool left to leave");
    assert_eq!(desk.adds.len(), 3);
    assert!(desk.edits.is_empty());
}

#[test]
fn adds_answered_out_of_order_settle_once_a_read_holds_them() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.holding = true;
    for [x, y] in [[10.0, 10.0], [30.0, 15.0], [30.0, 40.0]] {
        desk.frame(&click(x, y));
    }
    // Two adds, neither answered yet: the second's segment starts at the first's end all
    // the same (the tool holds what it added until a read settles it).
    let held = std::mem::take(&mut desk.held);
    assert_eq!(held.len(), 2);
    assert_eq!(held[1].1.lines[0].from, held[0].1.lines[0].to);
    // The core takes both; their answers come second first.
    let r1 = desk.apply(&held[0].1);
    let r2 = desk.apply(&held[1].1);
    desk.mode.tools.added(held[1].0, r2);
    desk.mode.tools.added(held[0].0, r1);
    desk.publish();
    // A read of r2 holds both: the chain goes on from (30, 40), where the second ended.
    desk.holding = false;
    desk.frame(&click(50.0, 40.0));
    let third = &desk.adds[2].lines[0];
    assert_eq!(third.from, held[1].1.lines[0].to);
    assert!(near(desk.place(&third.to), [50.0, 40.0]));
}

#[test]
fn a_refused_add_is_forgotten_and_its_chain_ends_there() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    for [x, y] in [[10.0, 10.0], [30.0, 15.0]] {
        desk.frame(&click(x, y));
    }
    desk.holding = true;
    desk.frame(&click(30.0, 40.0));
    let (token, _) = desk.held.remove(0);
    desk.mode.tools.refused(token);
    desk.holding = false;
    // The chain's last point was the refused add's: it's gone, so the chain ends, and the
    // next click begins one, adding nothing; the one after adds a segment from it.
    desk.frame(&click(50.0, 40.0));
    assert_eq!(desk.adds.len(), 1);
    desk.frame(&click(50.0, 60.0));
    assert_eq!(desk.adds.len(), 2);
    let line = &desk.adds[1].lines[0];
    assert!(near(desk.place(&line.from), [50.0, 40.0]));
    assert!(near(desk.place(&line.to), [50.0, 60.0]));
}

#[test]
fn an_add_another_client_undoes_is_forgotten_once_a_read_without_it_arrives() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    let before = desk.sketch.clone();
    for [x, y] in [[10.0, 10.0], [30.0, 15.0]] {
        desk.frame(&click(x, y));
    }
    assert_eq!(desk.adds.len(), 1);
    // Another client's undo: the session moves on to a revision whose sketch hasn't it.
    desk.sketch = before;
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    // The chain's last point is gone: its next click begins a chain, adding nothing.
    desk.frame(&click(50.0, 40.0));
    assert_eq!(desk.adds.len(), 1);
    assert!(desk.mode.tools.busy());
}

#[test]
fn ctrl_and_automatic_relations_off_add_no_horizontal_but_a_click_on_a_point_still_shares_it() {
    // (10, 10) to (30, 10.3): 0.86° off level, within the 2° the pointer shows `horizontal`
    // at, so the end is put level with the start, at (30, 10).
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&drag([10.0, 10.0], [30.0, 10.3]));
    let shown = &desk.adds[0];
    assert!(matches!(
        shown.constraints.as_slice(),
        [c] if matches!(c.form, Form::Horizontal { .. })
    ));
    assert!(near(desk.place(&shown.lines[0].to), [30.0, 10.0]));

    // With Ctrl held: no relation, the end where the pointer is.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&ctrl(drag([10.0, 10.0], [30.0, 10.3])));
    assert!(desk.adds[0].constraints.is_empty());
    assert!(near(desk.place(&desk.adds[0].lines[0].to), [30.0, 10.3]));

    // With Automatic relations off, the same.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.mode.settings.automatic_relations = false;
    desk.frame(&drag([10.0, 10.0], [30.0, 10.3]));
    assert!(desk.adds[0].constraints.is_empty());

    // Either way a click 0.2 mm from the sketch's point pa000001 (40, 40), within the pick,
    // shares it: the line ends on pa000001, adding one point, its start.
    for automatic in [true, false] {
        let mut sketch = empty();
        sketch.points.push(point("pa000001", 40.0, 40.0));
        let mut desk = Desk::new(Choice::Draw(ToolKind::Line), sketch);
        desk.mode.settings.automatic_relations = automatic;
        desk.frame(&ctrl(click(10.0, 20.0)));
        desk.frame(&ctrl(click(40.2, 40.0)));
        let add = &desk.adds[0];
        assert_eq!(add.lines[0].to, id("pa000001"));
        assert_eq!(add.points.len(), 1);
        assert!(add.constraints.is_empty(), "{:?}", add.constraints);
    }
}

#[test]
fn each_drawing_tool_adds_its_members() {
    // A line dragged from (10, 10) to (30, 25): two points and the line, at 36.9°, so no
    // relation.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&drag([10.0, 10.0], [30.0, 25.0]));
    let add = desk.adds[0].clone();
    assert_eq!(places(&desk, &add), [[10.0, 10.0], [30.0, 25.0]]);
    assert_eq!(sides(&desk, &add), [[[10.0, 10.0], [30.0, 25.0]]]);
    assert!(add.constraints.is_empty());

    // A centre rectangle, its centre clicked at (30, 20) and a corner at (40, 25): corners
    // (20, 15), (40, 15), (40, 25), (20, 25); its centre a construction point, with a
    // `symmetric` of two opposite corners about it (#278's C); two horizontal, two
    // vertical.
    let mut desk = Desk::new(Choice::Draw(ToolKind::CenterRectangle), empty());
    desk.frame(&click(30.0, 20.0));
    desk.frame(&click(40.0, 25.0));
    let add = desk.adds[0].clone();
    assert_eq!(
        places(&desk, &add),
        [
            [20.0, 15.0],
            [20.0, 25.0],
            [30.0, 20.0],
            [40.0, 15.0],
            [40.0, 25.0]
        ]
    );
    let centre = add.points.iter().find(|p| p.construction).unwrap();
    assert!(near(desk.place(&centre.id), [30.0, 20.0]));
    assert_eq!(add.points.iter().filter(|p| p.construction).count(), 1);
    let (a, b, c, d) = ([20.0, 15.0], [20.0, 25.0], [40.0, 15.0], [40.0, 25.0]);
    assert_eq!(sides(&desk, &add), [[a, b], [a, c], [b, d], [c, d]]);
    assert_eq!(held(&desk, &add, true), [[a, c], [b, d]]);
    assert_eq!(held(&desk, &add, false), [[a, b], [c, d]]);
    let symmetric: Vec<_> = add
        .constraints
        .iter()
        .filter_map(|k| match &k.form {
            Form::Symmetric {
                a: Reference::Element(a),
                b: Reference::Element(b),
                about: Reference::Element(about),
            } => Some((desk.place(a), desk.place(b), about.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(symmetric.len(), 1);
    let (p, q, about) = &symmetric[0];
    assert_eq!(about, &centre.id);
    assert!(near(
        [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0],
        [30.0, 20.0]
    ));
    assert!((p[0] - q[0]).abs() == 20.0 && (p[1] - q[1]).abs() == 10.0);
    assert_eq!(add.constraints.len(), 5);

    // A circle: its centre clicked at (20, 20), then (25, 20) on it: the centre point and a
    // circle of radius 5 about it.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Circle), empty());
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(25.0, 20.0));
    let add = desk.adds[0].clone();
    assert_eq!(places(&desk, &add), [[20.0, 20.0]]);
    let [SketchCircle { center, radius, .. }] = add.circles.as_slice() else {
        panic!("{add:?}");
    };
    assert_eq!(center, &add.points[0].id);
    assert!((radius.mm() - 5.0).abs() < 1e-12);
    assert!(add.lines.is_empty() && add.constraints.is_empty());

    // A centrepoint arc: its centre (20, 20), its start (30, 20), its end (20, 30): a
    // quarter of radius 10, counter-clockwise from the start.
    let mut desk = Desk::new(Choice::Draw(ToolKind::CenterpointArc), empty());
    for [x, y] in [[20.0, 20.0], [30.0, 20.0], [20.0, 30.0]] {
        desk.frame(&click(x, y));
    }
    let add = desk.adds[0].clone();
    let [arc] = add.arcs.as_slice() else {
        panic!("{add:?}");
    };
    assert!(near(desk.place(&arc.center), [20.0, 20.0]));
    assert!(near(desk.place(&arc.start), [30.0, 20.0]));
    assert!(near(desk.place(&arc.end), [20.0, 30.0]));
    assert_eq!(add.points.len(), 3);

    // A 3-point arc: its ends (10, 10) and (30, 10), then (20, 20) on it: its centre
    // (20, 10), radius 10, over the top, so counter-clockwise from (30, 10) to (10, 10).
    let mut desk = Desk::new(Choice::Draw(ToolKind::ThreePointArc), empty());
    for [x, y] in [[10.0, 10.0], [30.0, 10.0], [20.0, 20.0]] {
        desk.frame(&click(x, y));
    }
    let add = desk.adds[0].clone();
    let [arc] = add.arcs.as_slice() else {
        panic!("{add:?}");
    };
    assert!(near(desk.place(&arc.center), [20.0, 10.0]));
    assert!(near(desk.place(&arc.start), [30.0, 10.0]));
    assert!(near(desk.place(&arc.end), [10.0, 10.0]));

    // A tangent arc pressed on the end (20, 10) of the line na000001 from (10, 10) and
    // dragged to (30, 20): it leaves along +x, so its centre is (20, 20), radius 10,
    // counter-clockwise from (20, 10), tangent where they join.
    let mut sketch = empty();
    sketch.points = vec![point("pa000001", 10.0, 10.0), point("pa000002", 20.0, 10.0)];
    sketch.lines = vec![line("na000001", "pa000001", "pa000002")];
    let mut desk = Desk::new(Choice::Draw(ToolKind::TangentArc), sketch);
    desk.frame(&drag([20.0, 10.0], [30.0, 20.0]));
    let add = desk.adds[0].clone();
    let [arc] = add.arcs.as_slice() else {
        panic!("{add:?}");
    };
    assert_eq!(arc.start, id("pa000002"));
    assert!(near(desk.place(&arc.center), [20.0, 20.0]));
    assert!(near(desk.place(&arc.end), [30.0, 20.0]));
    let tangent = add
        .constraints
        .iter()
        .find_map(|k| match &k.form {
            Form::TangentAt { a, b, at, .. } => Some((a.clone(), b.clone(), at.clone())),
            _ => None,
        })
        .unwrap();
    let ends = [
        Reference::Element(id("na000001")),
        Reference::Element(arc.id.clone()),
    ];
    assert!(ends.contains(&tangent.0) && ends.contains(&tangent.1));
    assert_eq!(tangent.2, Reference::Element(id("pa000002")));

    // A point at each click, the tool staying; a click on one it placed adds nothing, and
    // says so (#278's D).
    let mut desk = Desk::new(Choice::Draw(ToolKind::Point), empty());
    desk.frame(&click(15.0, 7.0));
    let add = desk.adds[0].clone();
    assert_eq!(places(&desk, &add), [[15.0, 7.0]]);
    assert!(add.lines.is_empty() && add.constraints.is_empty());
    desk.frame(&click(15.0, 7.2));
    assert_eq!(desk.adds.len(), 1);
    let said = desk.said().unwrap().to_owned();
    assert!(said.contains("is already there"), "{said}");
    assert_eq!(
        desk.mode.tools.choice(),
        Some(Choice::Draw(ToolKind::Point))
    );
}

#[test]
fn construction_on_draws_the_tools_curves_as_construction_and_leaves_their_points() {
    // Construction (X) reaches the tool as its context's: a rectangle's four lines, a
    // circle, each `construction`, their points as made; the preview says so.
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.mode.settings.construction = true;
    desk.frame(&[
        ev(PointerAction::Press, 10.0, 10.0),
        ev(PointerAction::Move, 50.0, 30.0),
    ]);
    assert!(desk.mode.tools.preview().construction);
    desk.frame(&[ev(PointerAction::Release, 50.0, 30.0)]);
    let add = &desk.adds[0];
    assert_eq!(add.lines.len(), 4);
    assert!(add.lines.iter().all(|l| l.construction));
    assert!(add.points.iter().all(|p| !p.construction));
    let mut desk = Desk::new(Choice::Draw(ToolKind::Circle), empty());
    desk.mode.settings.construction = true;
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(25.0, 20.0));
    let add = &desk.adds[0];
    assert!(add.circles.iter().all(|c| c.construction) && add.circles.len() == 1);
    assert!(add.points.iter().all(|p| !p.construction));
    // Off, nothing is.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Circle), empty());
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(25.0, 20.0));
    assert!(!desk.adds[0].circles[0].construction);
}

/// na000001 from (0, 0) to (40, 0), na000002 from (20, −10) to (20, 10), crossing it at
/// (20, 0), and the circle ca000001 of radius 5 about (60, 0).
fn crossed() -> Sketch {
    let mut sketch = empty();
    sketch.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 40.0, 0.0),
        point("pa000003", 20.0, -10.0),
        point("pa000004", 20.0, 10.0),
        point("pa000005", 60.0, 0.0),
    ];
    sketch.lines = vec![
        line("na000001", "pa000001", "pa000002"),
        line("na000002", "pa000003", "pa000004"),
    ];
    sketch.circles = vec![SketchCircle {
        id: id("ca000001"),
        center: id("pa000005"),
        radius: silico_core::document::Coordinate::new(5.0).unwrap(),
        construction: false,
    }];
    sketch
}

#[test]
fn trim_to_closest_trims_the_curve_clicked_at_the_click_and_extends_the_way_a_drag_heads() {
    let mut desk = Desk::new(Choice::Trim, crossed());
    desk.mode.settings.trim = TrimMode::Closest;
    // A click 0.3 mm off na000001 at x = 30: within the pick, so it's trimmed there (the
    // core puts the pick on it).
    desk.frame(&click(30.0, 0.3));
    assert_eq!(
        desk.edits,
        [Request::Trim {
            curve: id("na000001"),
            pick: [30.0, 0.3]
        }]
    );
    // A drag from on na000001 back toward its start: extended at its start, to the next
    // curve.
    desk.edits.clear();
    desk.frame(&drag([30.0, 0.0], [25.0, 0.1]));
    assert_eq!(
        desk.edits,
        [Request::Extend {
            curve: id("na000001"),
            end: End::Start,
            by: ExtendBy::NextCurve,
            from: ExtendFrom::Trim,
        }]
    );
    // And toward its end: at its end.
    desk.edits.clear();
    desk.frame(&drag([30.0, 0.0], [35.0, 0.1]));
    assert!(matches!(
        desk.edits.as_slice(),
        [Request::Extend { end: End::End, .. }]
    ));
    // A drag from nowhere does nothing in Trim to closest, and says what it takes.
    desk.edits.clear();
    desk.frame(&drag([15.0, 8.0], [35.0, -2.0]));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(NOTHING_TO_TRIM));
}

#[test]
fn power_trim_trims_each_curve_a_drag_crosses_in_order_and_extends_one_dragged_along() {
    let mut desk = Desk::new(Choice::Trim, crossed());
    assert_eq!(desk.mode.settings.trim, TrimMode::Power);
    // From (15, 8) to (25, 3) to (35, −2): it crosses na000002 at x = 20 halfway along its
    // first step, at (20, 5.5); then na000001 at y = 0, three fifths along its second, at
    // (31, 0). Nothing is sent before the release.
    let path = [
        ev(PointerAction::Press, 15.0, 8.0),
        ev(PointerAction::Move, 25.0, 3.0),
    ];
    desk.frame(&path);
    assert!(desk.edits.is_empty());
    // The path is drawn, dotted, and the crossing so far marked.
    let preview = desk.mode.tools.preview().clone();
    assert_eq!(preview.guides, [[[15.0, 8.0], [25.0, 3.0]]]);
    assert_eq!(preview.points, [[20.0, 5.5]]);
    desk.frame(&[ev(PointerAction::Release, 35.0, -2.0)]);
    let [
        Request::Trim {
            curve: first,
            pick: p,
        },
        Request::Trim {
            curve: second,
            pick: q,
        },
    ] = desk.edits.as_slice()
    else {
        panic!("{:?}", desk.edits);
    };
    assert_eq!((first, second), (&id("na000002"), &id("na000001")));
    assert!(near(*p, [20.0, 5.5]), "{p:?}");
    assert!(near(*q, [31.0, 0.0]), "{q:?}");

    // A drag from on na000001 at (30, 0) along it to (45, 0.2): extended at its end, to
    // (45, 0), the pointer put on its line.
    desk.edits.clear();
    desk.frame(&drag([30.0, 0.0], [45.0, 0.2]));
    let x = silico_core::document::Coordinate::new(45.0).unwrap();
    let y = silico_core::document::Coordinate::new(0.0).unwrap();
    assert_eq!(
        desk.edits,
        [Request::Extend {
            curve: id("na000001"),
            end: End::End,
            by: ExtendBy::Point([x, y]),
            from: ExtendFrom::Trim,
        }]
    );
    // A drag from on the circle out across nothing else: a circle has no end to extend,
    // so it's a sweep, which crosses the circle where it starts, (65, 0), and trims it
    // there.
    desk.edits.clear();
    desk.frame(&drag([65.0, 0.0], [70.0, 3.0]));
    assert_eq!(
        desk.edits,
        [Request::Trim {
            curve: id("ca000001"),
            pick: [65.0, 0.0]
        }]
    );
    // In Trim to closest, a drag from the circle is an extend, which it has no end for:
    // said, nothing sent.
    desk.edits.clear();
    desk.mode.settings.trim = TrimMode::Closest;
    desk.frame(&drag([65.0, 0.0], [70.0, 3.0]));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(NO_END_TO_EXTEND));
    desk.mode.settings.trim = TrimMode::Power;
    // A click on nothing says so.
    desk.frame(&click(10.0, 30.0));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(NOTHING_TO_TRIM));
}

#[test]
fn a_cancelled_press_ends_the_gesture_with_nothing_sent() {
    // Power trim across both lines, cancelled before its release.
    let mut desk = Desk::new(Choice::Trim, crossed());
    desk.frame(&[
        ev(PointerAction::Press, 15.0, 8.0),
        ev(PointerAction::Move, 35.0, -2.0),
        ev(PointerAction::Cancel, 35.0, -2.0),
    ]);
    desk.frame(&[ev(PointerAction::Release, 35.0, -2.0)]);
    assert!(desk.edits.is_empty());
    assert!(!desk.mode.tools.busy());
    // A line's drag, cancelled: nothing added, the tool kept, and the next drag draws.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    let mut events = drag([10.0, 10.0], [30.0, 25.0]);
    events.pop();
    events.push(ev(PointerAction::Cancel, 30.0, 25.0));
    desk.frame(&events);
    assert!(desk.adds.is_empty());
    assert!(!desk.mode.tools.busy());
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    desk.frame(&drag([10.0, 10.0], [30.0, 25.0]));
    assert_eq!(desk.adds.len(), 1);
    // A chain's click cancelled (#365's `Event::Cancel`): as if never pressed, the chain
    // kept with its placed points, and the tool. Clicks at (10, 10) and (30, 15), then a
    // press at (50, 15) cancelled: the next click, at (30, 40), goes on from (30, 15).
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&click(10.0, 10.0));
    desk.frame(&click(30.0, 15.0));
    desk.frame(&[
        ev(PointerAction::Press, 50.0, 15.0),
        ev(PointerAction::Move, 50.0, 16.0),
        ev(PointerAction::Cancel, 50.0, 16.0),
    ]);
    assert_eq!(desk.adds.len(), 1);
    assert!(desk.mode.tools.busy());
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Draw(ToolKind::Line)));
    desk.frame(&click(30.0, 40.0));
    assert_eq!(desk.adds.len(), 2);
    assert_eq!(desk.adds[1].lines[0].from, desk.adds[0].lines[0].to);
    assert!(near(desk.place(&desk.adds[1].lines[0].to), [30.0, 40.0]));
    // A rectangle's second click cancelled: its first corner kept; the click after it
    // makes the rectangle from (10, 10).
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.frame(&click(10.0, 10.0));
    desk.frame(&[
        ev(PointerAction::Press, 60.0, 60.0),
        ev(PointerAction::Cancel, 60.0, 60.0),
    ]);
    assert!(desk.adds.is_empty());
    assert!(desk.mode.tools.busy());
    desk.frame(&click(50.0, 30.0));
    let members = desk.adds[0].clone();
    assert_rectangle(&desk, &members);
}

#[test]
fn split_splits_a_line_where_its_clicked_put_on_it_and_a_circle_at_two_clicks() {
    let mut desk = Desk::new(Choice::Split, crossed());
    // 0.3 mm above na000001 at x = 10: split at (10, 0), on it.
    desk.frame(&click(10.0, 0.3));
    assert_eq!(
        desk.edits,
        [Request::Split {
            curve: id("na000001"),
            at: vec![[10.0, 0.0]]
        }]
    );
    // The circle: its first click is held, drawn, and said; the second splits it at both.
    desk.edits.clear();
    desk.frame(&click(65.0, 0.0));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(SECOND_ON_CIRCLE));
    assert_eq!(desk.mode.tools.preview().points, [[65.0, 0.0]]);
    assert!(desk.mode.tools.busy());
    desk.frame(&click(60.0, 5.0));
    assert_eq!(
        desk.edits,
        [Request::Split {
            curve: id("ca000001"),
            at: vec![[65.0, 0.0], [60.0, 5.0]]
        }]
    );
    assert!(!desk.mode.tools.busy());
    // What it said of the first click is let go once it splits.
    assert_eq!(desk.said(), None);
    // A circle's first click, then one on a line: the line's split; the circle starts over.
    desk.edits.clear();
    desk.frame(&click(65.0, 0.0));
    desk.frame(&click(20.0, 5.0));
    assert_eq!(
        desk.edits,
        [Request::Split {
            curve: id("na000002"),
            at: vec![[20.0, 5.0]]
        }]
    );
    // Nothing there: said.
    desk.edits.clear();
    desk.frame(&click(10.0, 30.0));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(NOTHING_TO_SPLIT));
}

#[test]
fn pointers_that_arent_numbers_or_are_far_off_the_plane_add_nothing_and_say_why() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    let nan = Pos2::new(f32::NAN, f32::NAN);
    let odd = |action| PointerEvent {
        action,
        at: nan,
        shift: false,
        ctrl: false,
    };
    desk.frame_at(
        &[odd(PointerAction::Press), odd(PointerAction::Move)],
        Some(nan),
    );
    desk.frame_at(&[odd(PointerAction::Release)], Some(nan));
    assert!(desk.adds.is_empty());
    assert!(!desk.mode.tools.busy());
    // 1e9 mm out, past the kilometre a sketch is drawn in: said, nothing added.
    let far = Pos2::new(1.0e10, 500.0);
    let at_far = |action| PointerEvent {
        action,
        at: far,
        shift: false,
        ctrl: false,
    };
    desk.frame_at(
        &[at_far(PointerAction::Press), at_far(PointerAction::Release)],
        Some(far),
    );
    assert!(desk.adds.is_empty());
    assert!(desk.said().is_some_and(|said| said.contains("kilometre")));
    // Trim and split there find nothing.
    for choice in [Choice::Trim, Choice::Split] {
        let mut desk = Desk::new(choice, crossed());
        desk.frame_at(
            &[odd(PointerAction::Press), odd(PointerAction::Release)],
            Some(nan),
        );
        desk.frame_at(
            &[at_far(PointerAction::Press), at_far(PointerAction::Release)],
            Some(far),
        );
        assert!(desk.edits.is_empty());
    }
}

#[test]
fn a_tool_switched_mid_gesture_adds_nothing_and_a_read_mid_gesture_is_followed() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&[
        ev(PointerAction::Press, 10.0, 10.0),
        ev(PointerAction::Move, 30.0, 25.0),
    ]);
    assert!(desk.mode.tools.busy());
    desk.mode.tools.choose(Some(Choice::Draw(ToolKind::Circle)));
    assert!(!desk.mode.tools.busy());
    desk.frame(&[ev(PointerAction::Release, 30.0, 25.0)]);
    assert!(desk.adds.is_empty());
    // Choosing the tool that's on keeps its gesture.
    desk.frame(&click(20.0, 20.0));
    assert!(desk.mode.tools.busy());
    desk.mode.tools.choose(Some(Choice::Draw(ToolKind::Circle)));
    assert!(desk.mode.tools.busy());

    // A chain from the sketch's pa000001; a read arrives without it (another client removed
    // it): the chain ends at the next event, adding nothing.
    let mut sketch = empty();
    sketch.points.push(point("pa000001", 10.0, 10.0));
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), sketch);
    desk.frame(&click(10.0, 10.0));
    assert!(desk.mode.tools.busy());
    desk.sketch = empty();
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    desk.frame(&click(30.0, 30.0));
    assert!(desk.adds.is_empty());
    // A read that moves it: the chain's segment starts where the read puts it.
    let mut sketch = empty();
    sketch.points.push(point("pa000001", 10.0, 10.0));
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), sketch);
    desk.frame(&click(10.0, 10.0));
    desk.sketch.points[0] = point("pa000001", 12.0, 10.0);
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    desk.frame(&click(30.0, 30.0));
    assert_eq!(desk.adds[0].lines[0].from, id("pa000001"));
}

#[test]
fn no_tool_works_before_its_sketch_is_read_or_on_a_read_of_another() {
    let mut mode = SketchMode::new(1, Some(sketch_id()));
    mode.tools.choose(Some(Choice::Draw(ToolKind::Line)));
    let events = click(10.0, 10.0);
    with_painter(|painter| {
        mode.frame(
            painter,
            &FrameInput {
                events: &events,
                pointer: None,
                project: &hand,
                to_plane: &unhand,
            },
            &mut None,
        );
    });
    assert!(!mode.tools.busy());
    // A read of another sketch than the one open.
    let mut other = under(empty());
    other.sketch = FeatureId::new("s0000009").unwrap();
    mode.took(Read {
        model: Some(Revision::OPENED),
        result: Ok(other),
    });
    with_painter(|painter| {
        mode.frame(
            painter,
            &FrameInput {
                events: &events,
                pointer: None,
                project: &hand,
                to_plane: &unhand,
            },
            &mut None,
        );
    });
    assert!(!mode.tools.busy());
    assert!(mode.tools.take_requests().is_empty());
}

#[test]
fn with_a_tool_on_a_click_selects_nothing() {
    let mut sketch = empty();
    sketch.points.push(point("pa000001", 10.0, 10.0));
    let mut desk = Desk::new(Choice::Draw(ToolKind::Point), sketch);
    desk.frame(&click(10.0, 10.0));
    assert!(desk.mode.selection.is_empty());
    // Without one, the same click selects the point (11a's).
    desk.mode.tools.choose(None);
    desk.frame(&click(10.0, 10.0));
    assert_eq!(
        desk.mode.selected_ids().into_iter().collect::<Vec<_>>(),
        [id("pa000001")]
    );
}

#[test]
fn thousands_of_members_draw_trim_and_split_in_bounds() {
    // `n` vertical lines, x = 0.1 apart from 0, each from y = 0 to 10.
    let lines = |n: u32| {
        let mut sketch = empty();
        for i in 0..n {
            let x = f64::from(i) / 10.0;
            sketch.points.push(point(&format!("p{i:07}"), x, 0.0));
            sketch.points.push(point(&format!("q{i:07}"), x, 10.0));
            sketch.lines.push(line(
                &format!("n{i:07}"),
                &format!("p{i:07}"),
                &format!("q{i:07}"),
            ));
        }
        sketch
    };
    // A power trim drag across 2000 of them, at y = 5: each trimmed once, left to right.
    let mut desk = Desk::new(Choice::Trim, lines(2000));
    desk.frame(&drag([-1.0, 5.0], [201.0, 5.0]));
    assert_eq!(desk.edits.len(), 2000);
    for (i, request) in desk.edits.iter().enumerate() {
        let Request::Trim { curve, pick } = request else {
            panic!("{request:?}");
        };
        assert_eq!(curve, &id(&format!("n{i:07}")));
        assert!((pick[1] - 5.0).abs() < 1e-9);
    }
    // Split and trim to closest pick one among them: the one 0.02 mm off x = 100.
    for choice in [Choice::Split, Choice::Trim] {
        let mut desk = Desk::new(choice, lines(2000));
        desk.mode.settings.trim = TrimMode::Closest;
        desk.frame(&click(100.02, 5.0));
        let curve = match desk.edits.as_slice() {
            [Request::Split { curve, .. } | Request::Trim { curve, .. }] => curve.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(curve, id("n0001000"));
    }
    // A rectangle drawn among 250 (the gate takes a sketch of 1024 unknowns at most: their
    // 500 points' 1000, and the rectangle's four points' 8, fit).
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), lines(250));
    desk.frame(&drag([10.05, 20.0], [50.05, 40.0]));
    assert_eq!(desk.adds.len(), 1);
    assert_eq!(desk.adds[0].lines.len(), 4);
}

#[test]
fn the_crossings_of_a_step_with_a_line_and_a_round_are_where_they_meet() {
    let line = Curve::Line {
        from: [0.0, 0.0],
        to: [10.0, 0.0],
    };
    assert_eq!(
        crossings([5.0, -1.0], [5.0, 3.0], line),
        [(0.25, [5.0, 0.0])]
    );
    // Short of it, past its end, and along it: none.
    assert!(crossings([5.0, 1.0], [5.0, 3.0], line).is_empty());
    assert!(crossings([11.0, -1.0], [11.0, 1.0], line).is_empty());
    assert!(crossings([1.0, 0.0], [9.0, 0.0], line).is_empty());
    // A circle of radius 5 about the origin, crossed by y = 0 from −10 to 10: at x = −5 and
    // 5, a quarter and three quarters along.
    let circle = Curve::Round {
        centre: [0.0, 0.0],
        radius: 5.0,
        start: 0.0,
        sweep: TAU,
    };
    assert_eq!(
        crossings([-10.0, 0.0], [10.0, 0.0], circle),
        [(0.25, [-5.0, 0.0]), (0.75, [5.0, 0.0])]
    );
    // Its upper half as an arc (from 0 through π): the same step crosses it at both ends;
    // y = −3 crosses the circle but not the arc.
    let arc = Curve::Round {
        centre: [0.0, 0.0],
        radius: 5.0,
        start: 0.0,
        sweep: std::f64::consts::PI,
    };
    assert_eq!(crossings([-10.0, 0.0], [10.0, 0.0], arc).len(), 2);
    assert!(crossings([-10.0, -3.0], [10.0, -3.0], arc).is_empty());
    assert_eq!(crossings([-10.0, -3.0], [10.0, -3.0], circle).len(), 2);
    // A step of no length crosses nothing.
    assert!(crossings([5.0, 0.0], [5.0, 0.0], circle).is_empty());
}

#[test]
fn a_click_is_put_on_its_curve_exactly() {
    let line = Curve::Line {
        from: [0.0, 0.0],
        to: [40.0, 0.0],
    };
    assert_eq!(onto(line, [10.0, 0.3]), Some([10.0, 0.0]));
    let round = Curve::Round {
        centre: [60.0, 0.0],
        radius: 5.0,
        start: 0.0,
        sweep: TAU,
    };
    // 3-4-5: (63, 4) is on it; (66, 8), twice as far out, comes in to it.
    assert_eq!(onto(round, [66.0, 8.0]), Some([63.0, 4.0]));
    // Its centre is no one place of it.
    assert_eq!(onto(round, [60.0, 0.0]), None);
}

#[test]
fn the_canvas_menu_offers_end_chain_only_while_a_gesture_is_under_way() {
    use crate::sketch::bar::{MenuAction, canvas_menu};
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::cell::Cell;
    use std::rc::Rc;
    for busy in [true, false] {
        let asked = Rc::new(Cell::new(MenuAction::None));
        let seen = Rc::clone(&asked);
        let mut h = egui_kittest::Harness::new_ui(move |ui| {
            let action = canvas_menu(ui, busy);
            if action != MenuAction::None {
                seen.set(action);
            }
        });
        h.run();
        let end = h.get_by_label("End chain");
        assert_eq!(end.accesskit_node().is_disabled(), !busy);
        if busy {
            end.click();
            h.run();
            assert_eq!(asked.get(), MenuAction::EndChain);
        }
        h.get_by_label("Cancel sketch").click();
        h.run();
        assert_eq!(asked.get(), MenuAction::Cancel);
    }
}

#[test]
fn the_canvas_menu_is_the_prototypes_end_chain_smart_dimension_finish_and_cancel_with_their_keys() {
    // js/interact.js's `onContext` in a sketch: End chain ['Esc'], Smart Dimension
    // ('dimension' icon, ['D']), a separator, Finish Sketch ('check' icon, ['Mod', 'Enter']),
    // and Cancel sketch, in that order; Mod is Ctrl off macOS and ⌘ on it, and Enter ↵ (the
    // prototype's KEYNAME, app.js:8, as the registry names them). Each button named by its
    // words alone, its keys its shortcut for a screen reader, and drawn as every menu's item
    // is (`crate::menu`): its caps painted at its right, a separator line before Finish
    // Sketch, every item as wide as the widest. Smart Dimension asks for its tool.
    use crate::icons::Icon;
    use crate::sketch::bar::{MenuAction, canvas_menu, menu_items};
    use eframe::egui;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::cell::Cell;
    use std::rc::Rc;
    let ctx = egui::Context::default();
    ctx.set_os(egui::os::OperatingSystem::Nix);
    let items = menu_items(&ctx, false);
    let shown: Vec<_> = items
        .iter()
        .map(|item| {
            (
                item.action,
                item.icon,
                item.label,
                item.caps.clone(),
                item.off,
                item.separated,
            )
        })
        .collect();
    let caps = |caps: &[&str]| caps.iter().map(|cap| (*cap).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        shown,
        [
            (
                MenuAction::EndChain,
                None,
                "End chain",
                caps(&["Esc"]),
                Some("Nothing is being drawn"),
                false
            ),
            (
                MenuAction::Dimension,
                Some(Icon::Dimension),
                "Smart Dimension",
                caps(&["D"]),
                None,
                false
            ),
            (
                MenuAction::Finish,
                Some(Icon::Check),
                "Finish Sketch",
                caps(&["Ctrl", "↵"]),
                None,
                true
            ),
            (
                MenuAction::Cancel,
                None,
                "Cancel sketch",
                caps(&[]),
                None,
                false
            ),
        ]
    );
    ctx.set_os(egui::os::OperatingSystem::Mac);
    assert_eq!(menu_items(&ctx, true)[2].caps, caps(&["⌘", "↵"]));
    assert_eq!(menu_items(&ctx, true)[0].off, None);
    // Drawn: its buttons top to bottom in that order, each named by its words, its keys
    // its shortcut; Smart Dimension clicked asks for it.
    let asked = Rc::new(Cell::new(MenuAction::None));
    let seen = Rc::clone(&asked);
    let mut h = egui_kittest::Harness::new_ui(move |ui| {
        let action = canvas_menu(ui, true);
        if action != MenuAction::None {
            seen.set(action);
        }
    });
    h.ctx.set_os(egui::os::OperatingSystem::Nix);
    crate::theme::apply(&h.ctx);
    h.run();
    let mut buttons: Vec<(f32, String, Option<String>)> = h
        .query_all_by_role(egui::accesskit::Role::Button)
        .map(|node| {
            let node_rect = node.rect();
            let access = node.accesskit_node();
            (
                node_rect.top(),
                access.label().unwrap_or_default(),
                access.data().keyboard_shortcut().map(str::to_owned),
            )
        })
        .collect();
    buttons.sort_by(|a, b| a.0.total_cmp(&b.0));
    let named: Vec<(String, Option<String>)> = buttons
        .into_iter()
        .map(|(_, label, keys)| (label, keys))
        .collect();
    let keyed = |label: &str, keys: Option<&str>| (label.to_owned(), keys.map(str::to_owned));
    assert_eq!(
        named,
        [
            keyed("End chain", Some("Esc")),
            keyed("Smart Dimension", Some("D")),
            keyed("Finish Sketch", Some("Ctrl+↵")),
            keyed("Cancel sketch", None),
        ]
    );
    // Its caps painted (#444's review), each a cap's words: Esc, D, Ctrl and ↵.
    let painted: Vec<String> = crate::modal::tests::painted(&h)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    for cap in ["Esc", "D", "Ctrl", "↵"] {
        assert!(painted.iter().any(|text| text == cap), "{cap}: {painted:?}");
    }
    // Every item as wide as the widest, a menu's least at least (`crate::menu::width`).
    let widths: Vec<f32> = h
        .query_all_by_role(egui::accesskit::Role::Button)
        .map(|node| node.rect().width())
        .collect();
    assert_eq!(widths.len(), 4);
    assert!(
        widths.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-3),
        "{widths:?}"
    );
    assert!(widths[0] >= crate::menu::least_width() - 1e-3, "{widths:?}");
    // The widest item's own width where it's past the least (`crate::menu::item_width`'s
    // rule, the prototype's `.mi`), else the least.
    // Measured on a context of its own, in the app's theme (its fonts), as the harness's.
    let measuring = egui::Context::default();
    measuring.set_os(egui::os::OperatingSystem::Nix);
    crate::theme::apply(&measuring);
    let mut ui_widths: Vec<f32> = Vec::new();
    let mut measured = measuring.run_ui(Default::default(), |ui| {
        ui_widths = menu_items(ui.ctx(), true)
            .iter()
            .map(|item| crate::menu::item_width(ui, &item.line()))
            .collect();
    });
    measured.textures_delta.clear();
    let widest = ui_widths
        .iter()
        .copied()
        .fold(crate::menu::least_width(), f32::max);
    // Its widest item, Finish Sketch with its caps, is past a menu's least: the width is that
    // item's own, not the least.
    assert!(widest > crate::menu::least_width(), "{ui_widths:?}");
    assert!(
        (widths[0] - widest).abs() < 1e-3,
        "{widths:?} {ui_widths:?}"
    );
    // Drawn top to bottom, each 24 high, the separator (1 high, 4 above and below,
    // `.menu .msep`) between Smart Dimension and Finish Sketch alone.
    let mut tops: Vec<f32> = h
        .query_all_by_role(egui::accesskit::Role::Button)
        .map(|node| node.rect().top())
        .collect();
    tops.sort_by(f32::total_cmp);
    let gaps: Vec<f32> = tops.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(gaps, [24.0, 24.0 + 1.0 + 2.0 * 4.0, 24.0]);
    h.get_by_label("Smart Dimension").click();
    h.run();
    assert_eq!(asked.get(), MenuAction::Dimension);
}

#[test]
fn a_power_trim_sweep_pressed_near_a_line_trims_it_where_it_crosses_and_extends_nothing() {
    // Pressed 0.5 mm above na000001 at x = 30, within the pick, then swept down across it
    // to (30, −5): no extend (it left the line's carrier, and ends past neither end), but
    // the line's trim where the sweep crossed it, (30, 0).
    let mut desk = Desk::new(Choice::Trim, crossed());
    desk.frame(&drag([30.0, 0.5], [30.0, -5.0]));
    let [Request::Trim { curve, pick }] = desk.edits.as_slice() else {
        panic!("{:?}", desk.edits);
    };
    assert_eq!(curve, &id("na000001"));
    assert!(near(*pick, [30.0, 0.0]), "{pick:?}");
    // Along the line but short of its end, at (35, 0.2): no extend either; the sweep
    // crossed the line where it was pressed, (30, 0), so it's trimmed there.
    desk.edits.clear();
    desk.frame(&drag([30.0, 0.0], [35.0, 0.2]));
    assert!(matches!(
        desk.edits.as_slice(),
        [Request::Trim { curve, .. }] if *curve == id("na000001")
    ));
    // Out past its start, along it, to (−6, 0.3): its start extended to (−6, 0).
    desk.edits.clear();
    desk.frame(&drag([5.0, 0.0], [-6.0, 0.3]));
    let x = silico_core::document::Coordinate::new(-6.0).unwrap();
    let y = silico_core::document::Coordinate::new(0.0).unwrap();
    assert_eq!(
        desk.edits,
        [Request::Extend {
            curve: id("na000001"),
            end: End::Start,
            by: ExtendBy::Point([x, y]),
            from: ExtendFrom::Trim,
        }]
    );
    // Past its end but off its line on the way, through (45, 3): a sweep, which crosses
    // nothing past where it was pressed.
    desk.edits.clear();
    desk.frame(&[
        ev(PointerAction::Press, 35.0, 0.0),
        ev(PointerAction::Move, 45.0, 3.0),
        ev(PointerAction::Release, 45.0, 0.0),
    ]);
    assert!(
        desk.edits
            .iter()
            .all(|r| !matches!(r, Request::Extend { .. })),
        "{:?}",
        desk.edits
    );
}

/// The arc aa000001, a quarter of radius 10 about pa000001 (0, 0), counter-clockwise from
/// pa000002 (10, 0) to pa000003 (0, 10).
fn arced() -> Sketch {
    let mut sketch = empty();
    sketch.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 10.0, 0.0),
        point("pa000003", 0.0, 10.0),
    ];
    sketch.arcs = vec![silico_core::document::SketchArc {
        id: id("aa000001"),
        center: id("pa000001"),
        start: id("pa000002"),
        end: id("pa000003"),
        construction: false,
    }];
    sketch
}

/// (x, y) at `degrees` on the circle of radius 10 about the origin.
fn on_ten(degrees: f64) -> [f64; 2] {
    let a = degrees.to_radians();
    [10.0 * libm::cos(a), 10.0 * libm::sin(a)]
}

#[test]
fn trim_and_split_take_an_arc_where_its_clicked_crossed_or_dragged_past_its_end() {
    let half = 10.0 / std::f64::consts::SQRT_2;
    // Split: a click at (7.1, 7.1), 0.04 mm out, comes in to (10/√2, 10/√2), on it.
    let mut desk = Desk::new(Choice::Split, arced());
    desk.frame(&click(7.1, 7.1));
    let [Request::Split { curve, at }] = desk.edits.as_slice() else {
        panic!("{:?}", desk.edits);
    };
    assert_eq!(curve, &id("aa000001"));
    assert!(near(at[0], [half, half]), "{at:?}");
    assert_eq!(at.len(), 1);
    // Trim to closest: a click on it at (8, 6.1), 0.06 mm out, trims it there.
    let mut desk = Desk::new(Choice::Trim, arced());
    desk.mode.settings.trim = TrimMode::Closest;
    desk.frame(&click(8.0, 6.1));
    assert_eq!(
        desk.edits,
        [Request::Trim {
            curve: id("aa000001"),
            pick: [8.0, 6.1]
        }]
    );
    // Power trim: a sweep from (15, 5) to (5, 5) crosses it at (√75, 5), 30° round.
    let mut desk = Desk::new(Choice::Trim, arced());
    desk.frame(&drag([15.0, 5.0], [5.0, 5.0]));
    let [Request::Trim { curve, pick }] = desk.edits.as_slice() else {
        panic!("{:?}", desk.edits);
    };
    assert_eq!(curve, &id("aa000001"));
    assert!(near(*pick, [75.0_f64.sqrt(), 5.0]), "{pick:?}");
    // And a drag along it from 45° round past its end, through 80° to 110°: its end
    // extended to 110°, on its circle.
    desk.edits.clear();
    let [a, b, c] = [on_ten(45.0), on_ten(80.0), on_ten(110.0)];
    desk.frame(&[
        ev(PointerAction::Press, a[0], a[1]),
        ev(PointerAction::Move, b[0], b[1]),
        ev(PointerAction::Release, c[0], c[1]),
    ]);
    let [
        Request::Extend {
            curve,
            end: End::End,
            by: ExtendBy::Point([x, y]),
            from: ExtendFrom::Trim,
        },
    ] = desk.edits.as_slice()
    else {
        panic!("{:?}", desk.edits);
    };
    assert_eq!(curve, &id("aa000001"));
    // The release went through the hand projection's f32 points: within a micron.
    let to = [x.mm(), y.mm()];
    assert!((libm::hypot(to[0], to[1]) - 10.0).abs() < 1e-12, "{to:?}");
    assert!(
        (to[0] - c[0]).abs() < 1e-3 && (to[1] - c[1]).abs() < 1e-3,
        "{to:?}"
    );
    // Back before its start, through −20°: its start.
    desk.edits.clear();
    let [a, b] = [on_ten(30.0), on_ten(-20.0)];
    desk.frame(&[
        ev(PointerAction::Press, a[0], a[1]),
        ev(PointerAction::Move, on_ten(5.0)[0], on_ten(5.0)[1]),
        ev(PointerAction::Release, b[0], b[1]),
    ]);
    assert!(matches!(
        desk.edits.as_slice(),
        [Request::Extend {
            end: End::Start,
            ..
        }]
    ));
}

#[test]
fn construction_on_draws_an_arc_as_construction_and_leaves_its_points() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::ThreePointArc), empty());
    desk.mode.settings.construction = true;
    for [x, y] in [[10.0, 10.0], [30.0, 10.0], [20.0, 20.0]] {
        desk.frame(&click(x, y));
    }
    let [arc] = desk.adds[0].arcs.as_slice() else {
        panic!("{:?}", desk.adds);
    };
    assert!(arc.construction);
    assert!(desk.adds[0].points.iter().all(|p| !p.construction));
}

#[test]
fn a_corner_rectangle_with_shift_held_is_a_square_on_the_farther_offset() {
    // Dragged from (10, 10) to (50, 30) with Shift: 40 across, 20 up, so a 40 mm square
    // from (10, 10) to (50, 50).
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.frame(&shift(drag([10.0, 10.0], [50.0, 30.0])));
    let add = desk.adds[0].clone();
    let (a, b, c, d) = ([10.0, 10.0], [10.0, 50.0], [50.0, 10.0], [50.0, 50.0]);
    assert_eq!(places(&desk, &add), [a, b, c, d]);
    assert_eq!(sides(&desk, &add), [[a, b], [a, c], [b, d], [c, d]]);
    assert_eq!(held(&desk, &add, true), [[a, c], [b, d]]);
    assert_eq!(held(&desk, &add, false), [[a, b], [c, d]]);
}

#[test]
fn a_failed_read_mid_gesture_still_ends_the_press_at_its_release() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    desk.frame(&[ev(PointerAction::Press, 10.0, 10.0)]);
    assert!(desk.mode.tools.busy());
    // A read that failed: nothing to work against.
    desk.mode.took(Read {
        model: Some(desk.revision),
        result: Err(silico_core::command::Problems::from(
            silico_core::Error::new(silico_core::ErrorCode::InvalidInput, "a read that failed"),
        )),
    });
    desk.frame(&[
        ev(PointerAction::Move, 30.0, 25.0),
        ev(PointerAction::Release, 30.0, 25.0),
    ]);
    assert!(!desk.mode.tools.busy());
    // A good read, a hover, then a click at (40, 30): a chain begun there, nothing added
    // from the old press at (10, 10).
    desk.publish();
    desk.frame_at(&[], Some(at(40.0, 30.0)));
    desk.frame(&click(40.0, 30.0));
    assert!(desk.adds.is_empty());
    assert!(desk.mode.tools.busy());
    // Trim and split likewise.
    for choice in [Choice::Trim, Choice::Split] {
        let mut desk = Desk::new(choice, crossed());
        desk.frame(&[ev(PointerAction::Press, 10.0, 0.0)]);
        assert!(desk.mode.tools.busy());
        desk.mode.took(Read {
            model: Some(desk.revision),
            result: Err(silico_core::command::Problems::from(
                silico_core::Error::new(silico_core::ErrorCode::InvalidInput, "a read that failed"),
            )),
        });
        desk.frame(&[ev(PointerAction::Release, 10.0, 0.0)]);
        assert!(!desk.mode.tools.busy());
        desk.publish();
        desk.frame(&[ev(PointerAction::Release, 10.0, 0.0)]);
        assert!(desk.edits.is_empty());
    }
}

#[test]
fn a_long_power_trim_draws_only_its_trails_newest_places() {
    let mut desk = Desk::new(Choice::Trim, empty());
    let mut events = vec![ev(PointerAction::Press, 0.0, 0.0)];
    events.extend((1..=1000).map(|i| ev(PointerAction::Move, f64::from(i) / 10.0, 5.0)));
    desk.frame(&events);
    let guides = &desk.mode.tools.preview().guides;
    assert_eq!(guides.len(), TRAIL - 1);
    assert_eq!(guides.last(), Some(&[[99.9, 5.0], [100.0, 5.0]]));
}

/// A power trim drag through `path`, each place by its angle in degrees and its distance
/// from the origin: pressed at the first, moved through the rest, released at the last.
fn round_drag(desk: &mut Desk, path: &[(f64, f64)]) {
    let place = |(degrees, radius): (f64, f64)| {
        let a = f64::to_radians(degrees);
        [radius * libm::cos(a), radius * libm::sin(a)]
    };
    let mut events: Vec<PointerEvent> = Vec::new();
    for (i, &step) in path.iter().enumerate() {
        let [x, y] = place(step);
        let action = match i {
            0 => PointerAction::Press,
            _ if i == path.len() - 1 => PointerAction::Release,
            _ => PointerAction::Move,
        };
        events.push(ev(action, x, y));
    }
    desk.frame(&events);
}

#[test]
fn an_arc_dragged_round_past_its_start_extends_its_start_however_far_the_drag_turns() {
    // The quarter from 0° to 90°, pressed at 45° and dragged clockwise through 0°, −45°,
    // −90°, and −135° to −140°: 185° turned back from 45°, so past its start, though −140°
    // (220°) is nearer its end in angle.
    let mut desk = Desk::new(Choice::Trim, arced());
    let path = [45.0, 0.0, -45.0, -90.0, -135.0, -140.0].map(|d| (d, 10.0));
    round_drag(&mut desk, &path);
    assert!(
        matches!(
            desk.edits.as_slice(),
            [Request::Extend { curve, end: End::Start, .. }] if *curve == id("aa000001")
        ),
        "{:?}",
        desk.edits
    );
    // A 300° arc, from 0° to 300°, pressed at 20° and dragged 51° back to −31°: 31° past
    // its start, though −31° (329°) is nearer its end, 29° past it.
    let mut sketch = arced();
    let end = on_ten(300.0);
    sketch.points[2] = point("pa000003", end[0], end[1]);
    let mut desk = Desk::new(Choice::Trim, sketch);
    let path = [20.0, 10.0, 0.0, -15.0, -31.0].map(|d| (d, 10.0));
    round_drag(&mut desk, &path);
    assert!(
        matches!(
            desk.edits.as_slice(),
            [Request::Extend {
                end: End::Start,
                ..
            }]
        ),
        "{:?}",
        desk.edits
    );
}

#[test]
fn an_arc_drag_that_leaves_its_round_on_the_way_trims_where_it_crossed() {
    // Pressed on the quarter at 45°, out to radius 13 at 30°, in to radius 7 at 10°, and
    // back on its circle past its start at −10°: it left the round, so it's a sweep, which
    // crossed the arc (at its press, and on the way in near 20°): trimmed, not extended.
    let mut desk = Desk::new(Choice::Trim, arced());
    round_drag(
        &mut desk,
        &[(45.0, 10.0), (30.0, 13.0), (10.0, 7.0), (-10.0, 10.0)],
    );
    assert!(
        matches!(
            desk.edits.as_slice(),
            [Request::Trim { curve, .. }] if *curve == id("aa000001")
        ),
        "{:?}",
        desk.edits
    );
}

#[test]
fn a_drag_released_off_its_line_past_its_end_trims_where_it_crossed() {
    // Pressed on na000001 at (30, 0), along it to (38, 0.1), then released at (45, 3), past
    // its end but 3 mm off its line: a sweep, crossing the line where it was pressed.
    let mut desk = Desk::new(Choice::Trim, crossed());
    desk.frame(&[
        ev(PointerAction::Press, 30.0, 0.0),
        ev(PointerAction::Move, 38.0, 0.1),
        ev(PointerAction::Release, 45.0, 3.0),
    ]);
    assert_eq!(
        desk.edits,
        [Request::Trim {
            curve: id("na000001"),
            pick: [30.0, 0.0]
        }]
    );
}

// ---- Smart Dimension (M5.11c), through #345's API ----

/// A 40 × 20 rectangle from the origin: pa000001 (0, 0), pa000002 (40, 0), pa000003
/// (40, 20), pa000004 (0, 20), its sides na000001 to na000004 counter-clockwise from the
/// bottom, and no constraints.
fn boxed() -> Sketch {
    Sketch {
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
        ..empty()
    }
}

/// The Modify box's text, as Smart Dimension holds it.
fn modify_text(desk: &Desk) -> Option<String> {
    desk.mode
        .tools
        .dimension_preview()
        .and_then(|preview| preview.modify.clone())
}

/// The length the add `members` places: its line and its value, as the document writes it.
fn length_of(members: &Members) -> (Reference, String, bool) {
    match members.constraints.as_slice() {
        [c] => match &c.form {
            Form::Length {
                line,
                value,
                driving,
            } => (line.clone(), value.to_string(), *driving),
            other => panic!("not a length: {other:?}"),
        },
        other => panic!("not one constraint: {other:?}"),
    }
}

#[test]
fn smart_dimensions_modify_box_text_reaches_the_core_whole_and_what_it_reads_is_placed() {
    use crate::sketch::dimension::ModifyAction;
    use silico_core::sketch_tools::{Message, OfferChoice};
    let mut desk = Desk::new(Choice::Dimension, boxed());
    // The right side, na000002, (40, 0) to (40, 20), picked at (40, 10); then a click at
    // (55, 10), off everything: its length, 20, placed there, and the Modify box opened
    // holding it.
    desk.frame(&click(40.0, 10.0));
    assert!(modify_text(&desk).is_none());
    desk.frame(&click(55.0, 10.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("20"));
    assert_eq!(desk.mode.tools.modify_at(), Some([55.0, 10.0]));
    // The box stands at that click, as the frame projected it: (100 + 550, 500 − 100).
    let view = desk.mode.modify_view().unwrap();
    assert_eq!(view.at, Pos2::new(650.0, 400.0));
    assert_eq!((view.unit, view.placing, view.said), ("mm", true, None));
    let first = view.opened;
    // "25mm" reaches the core as typed; the core reads no number of it (SD9), says so, and
    // the box stays open with the text kept.
    desk.mode.modify(ModifyAction::Typed("25mm".to_owned()));
    assert_eq!(modify_text(&desk).as_deref(), Some("25mm"));
    desk.mode.modify(ModifyAction::Accept);
    desk.take();
    assert!(desk.adds.is_empty());
    let refused = Message::NotANumber.text();
    assert_eq!(desk.said(), Some(refused.as_str()));
    assert_eq!(
        desk.mode.modify_view().unwrap().said.as_deref(),
        Some(refused.as_str())
    );
    assert_eq!(modify_text(&desk).as_deref(), Some("25mm"));
    // " 25 ", its spaces and all, as typed: what's said goes with the old text, and the
    // core's reader takes JSON's spaces round the number, 25.
    desk.mode.modify(ModifyAction::Typed(" 25 ".to_owned()));
    assert_eq!(modify_text(&desk).as_deref(), Some(" 25 "));
    assert_eq!(desk.said(), None);
    desk.mode.modify(ModifyAction::Accept);
    desk.take();
    assert_eq!(desk.adds.len(), 1);
    assert_eq!(
        length_of(&desk.adds[0]),
        (Reference::Element(id("na000002")), "25".to_owned(), true)
    );
    // Its read leaves the freedoms where they were (the desk's reads all say 0), which the
    // core takes for a dependent dimension (SD7): its offer holds the tool, and a third
    // click places nothing but says why.
    assert!(desk.mode.offer().is_some());
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(20.0, 30.0));
    assert_eq!(desk.adds.len(), 1);
    assert!(modify_text(&desk).is_none());
    // Left driving: nothing asked, and the tool free for the next.
    assert!(desk.mode.answer_offer(OfferChoice::Leave));
    assert!(desk.mode.take_asks().is_empty());
    assert!(desk.mode.offer().is_none());
    // The top, na000003, 40 long, still picked: placed at (20, 30), and set to "1e1", an
    // exponent the document reads exactly, 10.
    desk.frame(&click(20.0, 30.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("40"));
    assert!(desk.mode.modify_view().unwrap().opened != first);
    desk.mode.modify(ModifyAction::Typed("1e1".to_owned()));
    desk.mode.modify(ModifyAction::Accept);
    desk.take();
    assert_eq!(desk.adds.len(), 2);
    assert_eq!(
        length_of(&desk.adds[1]),
        (Reference::Element(id("na000003")), "10".to_owned(), true)
    );
}

#[test]
fn an_offers_three_choices_ask_driven_against_the_revision_its_add_made_cancel_undo_leave_nothing()
{
    use crate::sketch::dimension::{Ask, ModifyAction};
    use silico_core::document::json::Value;
    use silico_core::sketch_tools::{OfferChoice, OfferRequest};
    let mut desk = Desk::new(Choice::Dimension, boxed());
    let place = |desk: &mut Desk, side: [f64; 2], at: [f64; 2]| {
        desk.frame(&click(side[0], side[1]));
        desk.frame(&click(at[0], at[1]));
        desk.mode.modify(ModifyAction::Accept);
        desk.take();
        let added = desk.adds.last().unwrap().constraints[0].id.clone();
        (added, desk.revision)
    };
    // Each placement offers (the desk's reads leave the freedoms at 0: dependent).
    let (added, made) = place(&mut desk, [40.0, 10.0], [55.0, 10.0]);
    assert_eq!(
        desk.mode.offer().map(|o| o.constraint.clone()),
        Some(added.clone())
    );
    assert!(desk.mode.answer_offer(OfferChoice::Driven));
    assert!(desk.mode.offer().is_none());
    match desk.mode.take_asks().as_slice() {
        [
            Ask::Offer(OfferRequest::SetField {
                revision,
                feature,
                field,
                value,
            }),
        ] => {
            assert_eq!(*revision, made);
            assert_eq!(*feature, sketch_id());
            assert_eq!(*field, format!("constraints.{added}.driving"));
            assert_eq!(value.value, Value::Bool(false));
        }
        other => panic!("{other:?}"),
    }
    let (_, made) = place(&mut desk, [20.0, 20.0], [20.0, 30.0]);
    assert!(desk.mode.answer_offer(OfferChoice::Cancel));
    assert_eq!(
        desk.mode.take_asks(),
        [Ask::Offer(OfferRequest::Undo {
            revision: made,
            to: None
        })]
    );
    // With none open, a choice answers nothing.
    assert!(!desk.mode.answer_offer(OfferChoice::Driven));
    assert!(desk.mode.take_asks().is_empty());
}

#[test]
fn a_pick_a_newer_read_no_longer_holds_is_dropped_and_an_open_box_on_it_closes_saying_why() {
    use silico_core::sketch_tools::Message;
    let mut desk = Desk::new(Choice::Dimension, boxed());
    desk.frame(&click(40.0, 10.0));
    let picks = |desk: &Desk| desk.mode.tools.dimension_preview().unwrap().picks.clone();
    assert_eq!(picks(&desk), [Reference::Element(id("na000002"))]);
    // Another client removes the right side mid-pick: the next read drops the pick.
    desk.sketch.lines.retain(|l| l.id != id("na000002"));
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    assert!(picks(&desk).is_empty());
    // A box open on the top when it goes closes, saying the dimension changed.
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(20.0, 30.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("40"));
    desk.sketch.lines.retain(|l| l.id != id("na000003"));
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    assert!(modify_text(&desk).is_none());
    assert!(desk.mode.modify_view().is_none());
    assert_eq!(desk.said(), Some(Message::DimensionChanged.text().as_str()));
    assert!(desk.adds.is_empty());
}

#[test]
fn a_right_click_locks_smart_dimensions_kind_and_a_pointer_off_the_plane_does_nothing() {
    let mut desk = Desk::new(Choice::Dimension, boxed());
    // The bottom-left corner and the top-right one, (0, 0) and (40, 20): with the pointer
    // inside the box they span, a slanted distance; locked there by a right-click, it stays
    // one with the pointer above the box, where it would be a horizontal one.
    desk.frame(&click(0.0, 0.0));
    desk.frame(&click(40.0, 20.0));
    desk.frame(&[ev(PointerAction::Move, 20.0, 10.0)]);
    let kind = |desk: &Desk| {
        let preview = desk.mode.tools.dimension_preview().unwrap();
        (
            preview.locked,
            preview.dimension.as_ref().map(|form| match form {
                Form::Distance { .. } => "distance",
                Form::DistanceX { .. } => "distance_x",
                Form::DistanceY { .. } => "distance_y",
                _ => "other",
            }),
        )
    };
    assert_eq!(kind(&desk), (false, Some("distance")));
    desk.frame(&[ev(PointerAction::Move, 20.0, 30.0)]);
    assert_eq!(kind(&desk), (false, Some("distance_x")));
    desk.frame(&[ev(PointerAction::Move, 20.0, 10.0)]);
    desk.frame(&[ev(PointerAction::RightClick, 20.0, 10.0)]);
    desk.frame(&[ev(PointerAction::Move, 20.0, 30.0)]);
    assert_eq!(kind(&desk), (true, Some("distance")));
    // A right-click where the plane gives no place is no event of the tool's.
    let before = kind(&desk);
    let nowhere = PointerEvent {
        action: PointerAction::RightClick,
        at: Pos2::new(f32::NAN, f32::NAN),
        shift: false,
        ctrl: false,
    };
    desk.frame_at(&[nowhere], None);
    assert_eq!(kind(&desk), before);
}

#[test]
fn a_cancelled_press_keeps_smart_dimensions_picks_and_leaves_no_press_held() {
    let mut desk = Desk::new(Choice::Dimension, boxed());
    let picks = |desk: &Desk| desk.mode.tools.dimension_preview().unwrap().picks.clone();
    // A press on nothing, cancelled (focus lost): nothing under way after it.
    desk.frame(&[ev(PointerAction::Press, 20.0, 10.0)]);
    assert!(desk.mode.tools.busy());
    desk.frame(&[ev(PointerAction::Cancel, 20.0, 10.0)]);
    assert!(!desk.mode.tools.busy());
    // The right side picked; a press on the top, cancelled: the pick kept, the top not
    // picked, and nothing placed.
    desk.frame(&click(40.0, 10.0));
    desk.frame(&[ev(PointerAction::Press, 20.0, 20.0)]);
    desk.frame(&[ev(PointerAction::Cancel, 20.0, 20.0)]);
    assert_eq!(picks(&desk), [Reference::Element(id("na000002"))]);
    assert!(modify_text(&desk).is_none());
    assert!(desk.adds.is_empty());
    // The next click places the right side's length, as it would have.
    desk.frame(&click(55.0, 10.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("20"));
}

#[test]
fn a_refused_dimension_add_frees_smart_dimension_for_the_next() {
    use crate::sketch::dimension::ModifyAction;
    let mut desk = Desk::new(Choice::Dimension, boxed());
    desk.holding = true;
    // The right side's length placed: its add sent, unanswered, holds the tool.
    desk.frame(&click(40.0, 10.0));
    desk.frame(&click(55.0, 10.0));
    desk.mode.modify(ModifyAction::Accept);
    desk.take();
    let [(token, _)] = desk.held.as_slice() else {
        panic!("{:?}", desk.held);
    };
    let token = *token;
    assert!(desk.mode.tools.busy());
    // Refused: forgotten, the tool free, and the next placement opens its box.
    desk.mode.tools.refused(token);
    assert!(!desk.mode.tools.busy());
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(20.0, 30.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("40"));
}

#[test]
fn smart_dimensions_boxes_count_down_from_the_top_and_a_reload_closes_the_one_open() {
    let mut desk = Desk::new(Choice::Dimension, boxed());
    // The first box opened is u64::MAX − 1, apart from placed dimensions' boxes, which count
    // up from 1.
    desk.frame(&click(40.0, 10.0));
    desk.frame(&click(55.0, 10.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("20"));
    assert_eq!(desk.mode.modify_view().unwrap().opened, u64::MAX - 1);
    // The session gone back to its file (a reload): the box closed, nothing under way, and
    // with the reload's read, none drawn.
    desk.mode.tools.forget_pending();
    assert_eq!(desk.mode.tools.modify_at(), None);
    assert!(!desk.mode.tools.busy());
    desk.publish();
    assert!(modify_text(&desk).is_none());
    assert!(desk.adds.is_empty());
    // The next is u64::MAX − 2.
    desk.frame(&click(20.0, 20.0));
    desk.frame(&click(20.0, 30.0));
    assert_eq!(modify_text(&desk).as_deref(), Some("40"));
    assert_eq!(desk.mode.modify_view().unwrap().opened, u64::MAX - 2);
}

#[test]
fn both_modify_boxes_read_the_same_text_the_same_way() {
    use crate::sketch::dimension::{ModifyAction, edit_value};
    // Each text typed in Smart Dimension's box, placing the right side's length, and given
    // to a placed length's box: the same value placed and sent, or the same words said.
    for text in [
        "25",
        " 25 ",
        "1e1",
        "25.000001",
        "1.2345678",
        "25mm",
        "abc",
        "true",
        "null",
        "\"25\"",
        "",
        "0",
        "-3",
        "1e7",
    ] {
        let mut desk = Desk::new(Choice::Dimension, boxed());
        desk.frame(&click(40.0, 10.0));
        desk.frame(&click(55.0, 10.0));
        let measured = desk
            .mode
            .tools
            .dimension_preview()
            .and_then(|p| p.dimension.clone())
            .unwrap();
        desk.mode.modify(ModifyAction::Typed(text.to_owned()));
        desk.holding = true;
        desk.mode.modify(ModifyAction::Accept);
        desk.take();
        let placed = match desk.held.as_slice() {
            [(_, members)] => match &members.constraints[0].form {
                Form::Length { value, .. } => Ok(value.to_string()),
                other => panic!("{other:?}"),
            },
            [] => Err(desk.said().unwrap_or_default().to_owned()),
            other => panic!("{other:?}"),
        };
        let sent = edit_value(&measured, text).map(|node| match node.value {
            silico_core::document::json::Value::Number(number) => number,
            other => panic!("{other:?}"),
        });
        // The gate's range (above 0, within reach) is the add's or the edit's to meet: a
        // value Smart Dimension's own check refuses there is still one the edit sends.
        match (&placed, &sent) {
            (Err(_), Ok(_)) if ["0", "-3", "1e7"].contains(&text) => {}
            _ => assert_eq!(placed, sent, "{text:?}"),
        }
    }
}

// ---- #364's mutants: the pick's edges, an arc's turn, and where a drag heads ----

use std::f64::consts::FRAC_PI_2;

/// The arc of radius 10 about the origin, counter-clockwise from `start` through `sweep`,
/// in radians.
fn ten(start: f64, sweep: f64) -> Curve {
    Curve::Round {
        centre: [0.0, 0.0],
        radius: 10.0,
        start,
        sweep,
    }
}

#[test]
fn a_pick_of_no_tolerance_takes_a_curve_the_place_is_exactly_on() {
    // (10, 0) lies on na000001, from (0, 0) to (40, 0), at a distance of exactly 0: a pick
    // of no tolerance takes it; one of less than none takes nothing.
    let read = under(crossed());
    let found = curve_at(&read, [10.0, 0.0], 0.0).map(|(id, kind, _)| (id, kind));
    assert_eq!(found, Some((id("na000001"), ElementKind::Line)));
    assert_eq!(curve_at(&read, [10.0, 0.0], -1.0), None);
    // A place 0.5 off it is outside a pick of none.
    assert_eq!(curve_at(&read, [10.0, 0.5], 0.0), None);
}

#[test]
fn a_place_past_an_arcs_end_is_on_no_curve_however_near_the_end() {
    // The quarter arc of `arced`, radius 10 about (0, 0) from (10, 0) to (0, 10), and a
    // pick of 0.7. (10, −0.5) is 0.5 from its start, but its foot on the round is at
    // −2.86°, past the sweep, so it's on no arc, as the core's pick (and so the selection)
    // finds curves; (−0.5, 10) likewise past its end, at 92.86°. (10, 0.5), at 2.86°, is
    // √100.25 − 10 = 0.0125 off it, and (0.5, 10) likewise: on it.
    let read = under(arced());
    let on = |at: [f64; 2]| curve_at(&read, at, 0.7).map(|(id, kind, _)| (id, kind));
    assert_eq!(on([10.0, -0.5]), None);
    assert_eq!(on([-0.5, 10.0]), None);
    let arc = Some((id("aa000001"), ElementKind::Arc));
    assert_eq!(on([10.0, 0.5]), arc);
    assert_eq!(on([0.5, 10.0]), arc);
}

#[test]
fn a_place_put_on_a_round_from_infinitely_far_or_onto_no_number_is_none() {
    let round = Curve::Round {
        centre: [0.0, 0.0],
        radius: 5.0,
        start: 0.0,
        sweep: TAU,
    };
    // (1.5e308, 1.5e308) is finite, but its distance from the centre, 2.1e308, isn't: no
    // direction to put it on the round along.
    assert_eq!(onto(round, [1.5e308, 1.5e308]), None);
    // A round of radius 1e308 about (0, 1e308): (0, 1.5e308) is 5e307 above its centre, so
    // it comes out to (0, 2e308), whose y is no number: none, though its x, 0, is one.
    let huge = Curve::Round {
        centre: [0.0, 1.0e308],
        radius: 1.0e308,
        start: 0.0,
        sweep: TAU,
    };
    assert_eq!(onto(huge, [0.0, 1.5e308]), None);
}

#[test]
fn a_step_crosses_an_arc_that_starts_off_the_x_axis_only_within_its_sweep() {
    // The arc of radius 5 about the origin from 90° through 90°, to 180°. y = 4 from
    // x = −10 to 0 meets the circle at (−3, 4), 126.87° round, on the arc, 0.7 along
    // (|(−10 + 10 t, 4)| = 5: t = 0.7 or 1.3).
    let arc = Curve::Round {
        centre: [0.0, 0.0],
        radius: 5.0,
        start: FRAC_PI_2,
        sweep: FRAC_PI_2,
    };
    let found = crossings([-10.0, 4.0], [0.0, 4.0], arc);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!((found[0].0 - 0.7).abs() < 1e-12, "{found:?}");
    assert!(near(found[0].1, [-3.0, 4.0]), "{found:?}");
    // y = −4 meets the circle at (−3, −4), 233.13° round: off the arc.
    assert!(crossings([-10.0, -4.0], [0.0, -4.0], arc).is_empty());
}

/// Tools with `choice` on, and the input of `read` with a pick of exactly `tolerance` mm.
fn input<'a>(read: &'a SketchRead, sketch: &'a FeatureId, tolerance: f64) -> Input<'a> {
    Input {
        read,
        sketch,
        revision: Revision::OPENED,
        tolerance,
        axes: Axes::BOTH,
        automatic_relations: true,
        construction: false,
        trim: TrimMode::Power,
    }
}

#[test]
fn a_trim_released_exactly_its_pick_from_its_press_is_a_click() {
    // na000001 alone, from (0, 0) to (40, 0); a pick of exactly 1 mm. Pressed at (5, 0)
    // and released 1 mm along at (6, 0), no farther than the pick: a click, trimming the
    // line at its press, (5, 0); never a drag along it (which would cross nothing).
    let mut sketch = empty();
    sketch.points = vec![point("pa000001", 0.0, 0.0), point("pa000002", 40.0, 0.0)];
    sketch.lines = vec![line("na000001", "pa000001", "pa000002")];
    let read = under(sketch);
    let feature = sketch_id();
    let input = input(&read, &feature, 1.0);
    let mut tools = Tools::default();
    tools.choose(Some(Choice::Trim));
    tools.handle(&input, Event::Press([5.0, 0.0]), Default::default());
    tools.handle(&input, Event::Release([6.0, 0.0]), Default::default());
    assert_eq!(
        tools.take_requests(),
        [Request::Trim {
            curve: id("na000001"),
            pick: [5.0, 0.0]
        }]
    );
    assert_eq!(tools.said(), None);
}

#[test]
fn a_drag_ends_past_a_lines_end_only_beyond_it() {
    let line = Curve::Line {
        from: [0.0, 0.0],
        to: [10.0, 0.0],
    };
    // At its end and at its start exactly: within it, past neither.
    assert_eq!(past_end(line, [5.0, 0.0], 0.0, [10.0, 0.0]), None);
    assert_eq!(past_end(line, [5.0, 0.0], 0.0, [0.0, 0.0]), None);
    // Beyond each.
    assert_eq!(past_end(line, [5.0, 0.0], 0.0, [11.0, 0.0]), Some(End::End));
    assert_eq!(
        past_end(line, [5.0, 0.0], 0.0, [-1.0, 0.0]),
        Some(End::Start)
    );
}

#[test]
fn a_drag_round_an_arc_ends_past_it_only_once_it_turns_beyond_its_sweep() {
    // A quarter from 0°, pressed at its start, (10, 0), 0 into it.
    let quarter = ten(0.0, FRAC_PI_2);
    // Turned exactly its sweep: at its end, not past it; not turned at all: at its start.
    assert_eq!(past_end(quarter, [10.0, 0.0], FRAC_PI_2, [0.0, 10.0]), None);
    assert_eq!(past_end(quarter, [10.0, 0.0], 0.0, [10.0, 0.0]), None);
    // A step either side: past its end, and before its start.
    assert_eq!(
        past_end(quarter, [10.0, 0.0], FRAC_PI_2 + 0.01, [0.0, 10.0]),
        Some(End::End)
    );
    assert_eq!(
        past_end(quarter, [10.0, 0.0], -0.01, [10.0, 0.0]),
        Some(End::Start)
    );
}

#[test]
fn a_press_just_past_an_arcs_end_is_at_its_end_and_one_just_before_its_start_at_its_start() {
    let deg = f64::to_radians;
    // The quarter from 0° to 90°, pressed 10° past its end at 100° (10° from its end, 260°
    // from its start): at its end, 90°. Turned back 5°, the drag is within it, at 85°.
    let quarter = ten(0.0, deg(90.0));
    assert_eq!(
        past_end(quarter, on_ten(100.0), -deg(5.0), on_ten(95.0)),
        None
    );
    // Turned on 5°: past its end, at 95°.
    assert_eq!(
        past_end(quarter, on_ten(100.0), deg(5.0), on_ten(105.0)),
        Some(End::End)
    );
    // Pressed at 350°, 10° before its start (260° past its end): at its start, 0°. Turned
    // on 5°, it's within it, at 5°; turned back 5°, before its start.
    assert_eq!(
        past_end(quarter, on_ten(350.0), deg(5.0), on_ten(355.0)),
        None
    );
    assert_eq!(
        past_end(quarter, on_ten(350.0), -deg(5.0), on_ten(345.0)),
        Some(End::Start)
    );
    // A 30° arc from 0°, pressed at 170°: 140° past its end, 190° before its start, so at
    // its end, 30°. Turned on 10°, to 40°: past its end. (At its start it'd be within it,
    // at 10°.)
    let thirty = ten(0.0, deg(30.0));
    assert_eq!(
        past_end(thirty, on_ten(170.0), deg(10.0), on_ten(180.0)),
        Some(End::End)
    );
    // An arc whose start isn't 0°: from 90° through 90°, pressed at 100°, 10° into it.
    // Turned back 20°, to −10°: before its start.
    let second = ten(deg(90.0), deg(90.0));
    assert_eq!(
        past_end(second, on_ten(100.0), -deg(20.0), on_ten(80.0)),
        Some(End::Start)
    );
}

#[test]
fn a_press_exactly_midway_past_both_ends_of_an_arc_is_at_its_start() {
    // The arc from 2π − 5 rad through 10 − 2π = 3.7168 rad, pressed at angle 0, which is
    // 5 rad round from its start: 5 − 3.7168 = 1.2832 past its end, and 2π − 5 = 1.2832
    // before its start, exactly as far each way. The tie goes to its start: turned back
    // 0.1, the drag is before it; on 0.1, within it.
    let start = TAU - 5.0;
    let sweep = 5.0 - (TAU - 5.0);
    assert_eq!(5.0 - sweep, TAU - 5.0, "the tie, exactly");
    let arc = ten(start, sweep);
    assert_eq!(
        past_end(arc, [10.0, 0.0], -0.1, [10.0, 0.0]),
        Some(End::Start)
    );
    assert_eq!(past_end(arc, [10.0, 0.0], 0.1, [10.0, 0.0]), None);
}

#[test]
fn a_drag_across_a_line_or_out_from_a_rounds_centre_heads_for_its_start() {
    let line = Curve::Line {
        from: [0.0, 0.0],
        to: [10.0, 0.0],
    };
    // Along it, its end; against it, its start; straight across it, neither way along: its
    // start.
    assert_eq!(headed(line, [5.0, 0.0], [6.0, 0.0]), End::End);
    assert_eq!(headed(line, [5.0, 0.0], [4.0, 0.0]), End::Start);
    assert_eq!(headed(line, [5.0, 0.0], [5.0, 3.0]), End::Start);
    let quarter = ten(0.0, FRAC_PI_2);
    // Round it counter-clockwise, its end; clockwise, its start; straight out from its
    // centre, neither way round: its start.
    assert_eq!(headed(quarter, [10.0, 0.0], [0.0, 10.0]), End::End);
    assert_eq!(headed(quarter, [0.0, 10.0], [10.0, 0.0]), End::Start);
    assert_eq!(headed(quarter, [10.0, 0.0], [20.0, 0.0]), End::Start);
}

#[test]
fn a_split_click_on_a_second_circle_begins_again_there() {
    // ca000001, radius 5 about (60, 0), and ca000002, radius 5 about (80, 0).
    let mut sketch = crossed();
    sketch.points.push(point("pa000006", 80.0, 0.0));
    sketch.circles.push(SketchCircle {
        id: id("ca000002"),
        center: id("pa000006"),
        radius: silico_core::document::Coordinate::new(5.0).unwrap(),
        construction: false,
    });
    let mut desk = Desk::new(Choice::Split, sketch);
    desk.frame(&click(65.0, 0.0));
    // Its second click on the other circle splits neither: that circle's first place.
    desk.frame(&click(85.0, 0.0));
    assert!(desk.edits.is_empty(), "{:?}", desk.edits);
    assert_eq!(desk.said(), Some(SECOND_ON_CIRCLE));
    assert_eq!(desk.mode.tools.preview().points, [[85.0, 0.0]]);
    // And a second on it splits it at both.
    desk.frame(&click(80.0, 5.0));
    assert_eq!(
        desk.edits,
        [Request::Split {
            curve: id("ca000002"),
            at: vec![[85.0, 0.0], [80.0, 5.0]]
        }]
    );
}

#[test]
fn a_pointer_half_a_number_is_off_the_plane_and_reaches_no_tool() {
    // x no number, y one; then the other way round. The tool hears of neither: nothing
    // said, nothing begun, no place given.
    let mut desk = Desk::new(Choice::Draw(ToolKind::Line), empty());
    for at in [Pos2::new(f32::NAN, 500.0), Pos2::new(100.0, f32::NAN)] {
        let press = PointerEvent {
            action: PointerAction::Press,
            at,
            shift: false,
            ctrl: false,
        };
        desk.frame_at(&[press], None);
        assert_eq!(desk.said(), None);
        assert!(!desk.mode.tools.busy());
        assert_eq!(desk.mode.tools.last(), None);
    }
    assert!(desk.adds.is_empty());
}

#[test]
fn an_arcs_distance_is_its_rims_on_its_sweep_and_a_place_past_it_is_on_no_arc() {
    // About (0, 0), radius 10, from 90° through 90°: from (0, 10) to (−10, 0).
    let mut sketch = empty();
    sketch.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 0.0, 10.0),
        point("pa000003", -10.0, 0.0),
    ];
    sketch.arcs = vec![silico_core::document::SketchArc {
        id: id("aa000001"),
        center: id("pa000001"),
        start: id("pa000002"),
        end: id("pa000003"),
        construction: false,
    }];
    let read = under(sketch);
    let on = |at: [f64; 2], tolerance: f64| curve_at(&read, at, tolerance).map(|(id, ..)| id);
    // At 135°, 1 outside the rim: 1 off it, within 1.01 and not 0.99.
    let out = 11.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(on([-out, out], 1.01), Some(id("aa000001")));
    assert_eq!(on([-out, out], 0.99), None);
    // (−10, −1), just past its end, 1 from (−10, 0): its foot at 185.7°, past the sweep, so
    // on no arc at any tolerance (the core's rule, as a selection finds curves).
    assert_eq!(on([-10.0, -1.0], 1.5), None);
    assert_eq!(on([-10.0, -1.0], 100.0), None);
}

/// A line na000001 from (0, 0) to (10, 0); a wall, nb000001, along x = 30 from (30, −10) to
/// (30, 10); a circle ca000001 of radius 5 about (60, 0); and a lone point, pd000001, at
/// (20, 20).
fn walled() -> Sketch {
    let mut sketch = empty();
    sketch.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 10.0, 0.0),
        point("pb000001", 30.0, -10.0),
        point("pb000002", 30.0, 10.0),
        point("pc000001", 60.0, 0.0),
        point("pd000001", 20.0, 20.0),
    ];
    sketch.lines = vec![
        line("na000001", "pa000001", "pa000002"),
        line("nb000001", "pb000001", "pb000002"),
    ];
    sketch.circles = vec![SketchCircle {
        id: id("ca000001"),
        center: id("pc000001"),
        radius: silico_core::document::Coordinate::new(5.0).unwrap(),
        construction: false,
    }];
    sketch
}

/// The pointer moved to (x, y) with no button held: a hover.
fn hover(x: f64, y: f64) -> Vec<PointerEvent> {
    vec![ev(PointerAction::Move, x, y)]
}

#[test]
fn extend_previews_the_half_hovered_and_a_click_asks_that_end_to_the_next_curve() {
    let mut desk = Desk::new(Choice::Extend, walled());
    // Over na000001 at (8, 0.2), past its midpoint (5, 0): its end, (10, 0), drawn on along
    // y = 0 to the wall at (30, 0); nothing said, nothing asked.
    desk.frame(&hover(8.0, 0.2));
    let preview = desk.mode.tools.preview().clone();
    assert_eq!(preview.lines.len(), 1, "{preview:?}");
    assert!(near(preview.lines[0][0], [10.0, 0.0]), "{preview:?}");
    assert!(near(preview.lines[0][1], [30.0, 0.0]), "{preview:?}");
    assert_eq!(desk.said(), None);
    assert!(desk.edits.is_empty());
    // A click there: one `sketch_extend` of that end to the next curve, its step Extend's.
    // The tool stays on.
    desk.frame(&click(8.0, 0.2));
    assert_eq!(
        desk.edits,
        [Request::Extend {
            curve: id("na000001"),
            end: End::End,
            by: ExtendBy::NextCurve,
            from: ExtendFrom::Extend,
        }]
    );
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Extend));
    assert!(!desk.mode.tools.busy());
    // Over (2, −0.2), before the midpoint: its start, (0, 0), which meets nothing to the
    // left. No extension drawn; the core's words said.
    desk.edits.clear();
    desk.frame(&hover(2.0, -0.2));
    let preview = desk.mode.tools.preview();
    assert!(
        preview.lines.is_empty() && preview.arcs.is_empty(),
        "{preview:?}"
    );
    let nothing = "line na000001 meets no other curve past its start";
    assert_eq!(desk.said(), Some(nothing));
    // A click there asks nothing, and says so.
    desk.frame(&click(2.0, -0.2));
    assert!(desk.edits.is_empty());
    assert_eq!(desk.said(), Some(nothing));
    assert_eq!(desk.mode.tools.choice(), Some(Choice::Extend));
    // Back over the end's half: shown again, what was said let go.
    desk.frame(&hover(9.0, 0.0));
    assert_eq!(desk.mode.tools.preview().lines.len(), 1);
    assert_eq!(desk.said(), None);
}

#[test]
fn extend_takes_nothing_from_a_circle_a_point_or_an_axis() {
    let mut desk = Desk::new(Choice::Extend, walled());
    // On the circle's rim at (65, 0.2), on the point at (20, 20), and on the y axis at
    // (0.2, 20), clear of every curve: no extension, nothing said.
    for [x, y] in [[65.0, 0.2], [20.0, 20.0], [0.2, 20.0]] {
        desk.frame(&hover(x, y));
        let preview = desk.mode.tools.preview();
        assert!(
            preview.lines.is_empty() && preview.arcs.is_empty() && preview.points.is_empty(),
            "({x}, {y}): {preview:?}"
        );
        assert_eq!(desk.said(), None, "({x}, {y})");
    }
    // Clicked: nothing asked; a circle has no end, and the rest is nothing to extend.
    desk.frame(&click(65.0, 0.2));
    assert_eq!(desk.said(), Some(NO_END_TO_EXTEND));
    desk.frame(&click(20.0, 20.0));
    assert_eq!(desk.said(), Some(NOTHING_TO_EXTEND));
    desk.frame(&click(0.2, 20.0));
    assert_eq!(desk.said(), Some(NOTHING_TO_EXTEND));
    assert!(desk.edits.is_empty());
}

#[test]
fn extend_previews_an_arc_round_its_circle_from_the_end_hovered() {
    // `arced`'s quarter, about (0, 0), radius 10, from 0° to 90°, and a ledge along y = 5
    // from (−20, 5) to (−1, 5), which the circle meets at 150°, (−√75, 5).
    let mut sketch = arced();
    sketch.points.push(point("pb000001", -20.0, 5.0));
    sketch.points.push(point("pb000002", -1.0, 5.0));
    sketch.lines = vec![line("na000001", "pb000001", "pb000002")];
    let ledge = [-(75.0_f64.sqrt()), 5.0];
    let mut desk = Desk::new(Choice::Extend, sketch);
    // At 80°, past the mid-angle: the end, from (0, 10) counter-clockwise on to the ledge.
    let [x, y] = on_ten(80.0);
    desk.frame(&hover(x, y));
    let arcs = desk.mode.tools.preview().arcs.clone();
    assert_eq!(arcs.len(), 1);
    assert!(near(arcs[0].center, [0.0, 0.0]));
    assert!(near(arcs[0].start, [0.0, 10.0]), "{arcs:?}");
    assert!(near(arcs[0].end, ledge), "{arcs:?}");
    desk.frame(&click(x, y));
    assert!(matches!(
        desk.edits.as_slice(),
        [Request::Extend { curve, end: End::End, by: ExtendBy::NextCurve, .. }] if *curve == id("aa000001")
    ));
    // At 10°, before it: the start, clockwise 210° round to the same place, drawn
    // counter-clockwise from there on to (10, 0). (The first extend refused, so the tool
    // clicks again on the read it has.)
    desk.edits.clear();
    desk.mode.tools.extend_refused();
    let [x, y] = on_ten(10.0);
    desk.frame(&hover(x, y));
    let arcs = desk.mode.tools.preview().arcs.clone();
    assert!(near(arcs[0].start, ledge), "{arcs:?}");
    assert!(near(arcs[0].end, [10.0, 0.0]), "{arcs:?}");
    desk.frame(&click(x, y));
    assert!(matches!(
        desk.edits.as_slice(),
        [Request::Extend {
            end: End::Start,
            by: ExtendBy::NextCurve,
            ..
        }]
    ));
}

#[test]
fn a_read_after_an_extend_shows_the_hover_of_the_newest_sketch() {
    let mut desk = Desk::new(Choice::Extend, walled());
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 1);
    // The extend done, as the core does it: na000001's end at (30, 0). The read of it, the
    // pointer still: the line's midpoint now (15, 0), so (8, 0.2) is its start's half,
    // which meets nothing; no extension drawn, and said.
    let end = desk
        .sketch
        .points
        .iter_mut()
        .find(|p| p.id == id("pa000002"))
        .unwrap();
    end.x = silico_core::document::Coordinate::new(30.0).unwrap();
    desk.revision = desk.revision.next().unwrap();
    desk.publish();
    let preview = desk.mode.tools.preview();
    assert!(preview.lines.is_empty(), "{preview:?}");
    assert_eq!(
        desk.said(),
        Some("line na000001 meets no other curve past its start")
    );
}

#[test]
fn a_second_click_before_the_read_of_the_first_extend_asks_nothing() {
    // A wall at x = 20 too: the first click at (9, 0) extends na000001's end to it, (20, 0).
    // Clicked again before any read shows that, the read is stale: on it the second click
    // would plan the same end again, though the line now reaches the wall, and go on to the
    // wall at x = 30, which no preview showed. So it asks nothing; one request in all.
    let mut sketch = walled();
    sketch.points.push(point("pe000001", 20.0, -10.0));
    sketch.points.push(point("pe000002", 20.0, 10.0));
    sketch.lines.push(line("ne000001", "pe000001", "pe000002"));
    let mut desk = Desk::new(Choice::Extend, sketch);
    let mut twice = click(9.0, 0.0);
    twice.extend(click(9.0, 0.0));
    desk.frame(&twice);
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
    // And over two frames, with no read between them.
    desk.edits.clear();
    let mut desk = Desk::new(Choice::Extend, desk.sketch.clone());
    desk.frame(&click(9.0, 0.0));
    desk.frame(&click(9.0, 0.0));
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
}

#[test]
fn the_extends_refusal_or_a_read_of_what_it_made_lets_extend_click_again() {
    let mut desk = Desk::new(Choice::Extend, walled());
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 1);
    // A read of the same revision again (one already asked) doesn't let it go.
    desk.publish();
    desk.frame(&click(8.0, 0.3));
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
    // The core's refusal does: the next click asks again.
    desk.mode.tools.extend_refused();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 2, "{:?}", desk.edits);
    // And so does its answer, made at the next revision, once a read of that revision comes
    // (its sketch as it was, for the test, so the click plans the same extend again).
    desk.revision = desk.revision.next().unwrap();
    desk.mode.tools.extend_made(desk.revision);
    desk.frame(&click(8.0, 0.3));
    assert_eq!(
        desk.edits.len(),
        2,
        "made, but no read holds it yet: {:?}",
        desk.edits
    );
    desk.publish();
    desk.frame(&click(8.0, 0.3));
    assert_eq!(desk.edits.len(), 3, "{:?}", desk.edits);
    // Hovering went on all the while.
    assert_eq!(desk.mode.tools.preview().lines.len(), 1);
}

#[test]
fn only_a_read_at_or_past_the_revision_an_extend_made_lets_it_go() {
    let r0 = Revision::OPENED;
    let r1 = r0.next().unwrap();
    let r2 = r1.next().unwrap();
    let r3 = r2.next().unwrap();
    let mut desk = Desk::new(Choice::Extend, walled());
    // Planned on r0's read. Before any answer, a read of r1 (an undo's, sent before it):
    // newer than the read it was planned on, but nothing says it holds the extend. Held.
    desk.frame(&click(8.0, 0.2));
    desk.revision = r1;
    desk.publish();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
    // Its answer made r2.
    desk.mode.tools.extend_made(r2);
    // r1's read: newer than the read it was planned on, older than what it made. Held.
    desk.revision = r1;
    desk.publish();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
    // r2's: let go.
    desk.revision = r2;
    desk.publish();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 2, "{:?}", desk.edits);
    // That one made r3; a read at r0, older than any, comes late: held still.
    desk.mode.tools.extend_made(r3);
    desk.revision = r0;
    desk.publish();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 2, "{:?}", desk.edits);
    // Refused, it's let go; the next click's extend makes r4, whose read comes before its
    // answer, and a late read of r0 after that: r4 was seen, so the answer lets it go at
    // once.
    desk.mode.tools.extend_refused();
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 3);
    let r4 = r3.next().unwrap();
    desk.revision = r4;
    desk.publish();
    desk.revision = r0;
    desk.publish();
    // A click on that late read before the answer: still held.
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 3);
    desk.mode.tools.extend_made(r4);
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 4, "{:?}", desk.edits);
}

#[test]
fn a_press_dropped_between_two_clicks_on_one_read_still_asks_one_edit() {
    let mut desk = Desk::new(Choice::Extend, walled());
    desk.frame(&click(8.0, 0.2));
    // A press, then dropped with no read to give it (`cancel(None)`, as a release off the
    // plane with no read is), then a click: still on the first click's read.
    desk.frame(&[ev(PointerAction::Press, 8.0, 0.2)]);
    desk.mode.tools.cancel(None);
    assert!(!desk.mode.tools.busy());
    desk.frame(&click(8.0, 0.2));
    assert_eq!(desk.edits.len(), 1, "{:?}", desk.edits);
}

#[test]
fn an_extend_press_released_past_the_pick_sends_nothing_and_escape_clears_its_preview() {
    let mut desk = Desk::new(Choice::Extend, walled());
    // Pressed at (8, 0.2) and released at (8, 2): 1.8 mm off, past the 0.7 mm pick, so no
    // click. The hover at the release is off every curve: nothing drawn.
    desk.frame(&[
        ev(PointerAction::Press, 8.0, 0.2),
        ev(PointerAction::Release, 8.0, 2.0),
    ]);
    assert!(desk.edits.is_empty());
    assert!(!desk.mode.tools.busy());
    // Hovered over the line, its extension drawn; Esc leaves the tool, and the preview with
    // it.
    desk.frame(&hover(8.0, 0.2));
    assert_eq!(desk.mode.tools.preview().lines.len(), 1);
    assert!(desk.mode.escape_tool());
    assert_eq!(desk.mode.tools.choice(), None);
    assert_eq!(desk.mode.tools.preview(), &Preview::default());
}
