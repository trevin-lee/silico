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
            mode.frame(painter, &input);
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
fn construction_on_draws_the_tools_curves_as_construction() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::CornerRectangle), empty());
    desk.mode.settings.construction = true;
    desk.frame(&drag([10.0, 10.0], [50.0, 30.0]));
    let add = &desk.adds[0];
    assert_eq!(add.lines.len(), 4);
    assert!(add.lines.iter().all(|l| l.construction));
    assert!(add.points.iter().all(|p| !p.construction));
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
            by: ExtendBy::NextCurve
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
            by: ExtendBy::Point([x, y])
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
            by: ExtendBy::Point([x, y])
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
fn construction_on_draws_an_arc_as_construction() {
    let mut desk = Desk::new(Choice::Draw(ToolKind::ThreePointArc), empty());
    desk.mode.settings.construction = true;
    for [x, y] in [[10.0, 10.0], [30.0, 10.0], [20.0, 20.0]] {
        desk.frame(&click(x, y));
    }
    let [arc] = desk.adds[0].arcs.as_slice() else {
        panic!("{:?}", desk.adds);
    };
    assert!(arc.construction);
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
