//! M5.11's sketch tools (silico_core::sketch_tools) on the real core: each gesture driven
//! against the session's newest `sketch` read, its `sketch_add` sent through the sessions'
//! own command, and the sketch the rebuild then solves on PlaneGCS, in this crate's worker,
//! read back. So the core takes what the tools make, and the sketch solves as worked by hand:
//! each figure's degrees of freedom counted from its unknowns (two a point, an arc's own rules
//! in PlaneGCS's terms: a free arc has five) less its equations, its solved points where the
//! tool drew them (the tools draw what their relations hold), and its profile's regions.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::sketch::SketchRead;
use silico_core::command::{Call, ModelChoice, NewMembers, Revision, SessionId, Sessions};
use silico_core::document::sketch_edit::Members;
use silico_core::document::{ElementId, FeatureId, Form, Length, Reference, SketchConstraint};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use silico_core::sketch_states::{ConstraintState, SketchState};
use silico_core::sketch_tools::{
    Context, DimensionEvent, Event, Modifiers, NewIds, Offer, OfferChoice, OfferRequest, Placement,
    PlacementKind, RelateEdit, RelationTool, SketchTool, SmartDimension, ToolKind, offer,
    read_typed, relate,
};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// The pick tolerance, in millimetres.
const TOLERANCE: f64 = 0.5;

/// IDs `t0000001`, `t0000002`, ... in turn, skipping any taken.
struct Counter(u32);

impl NewIds for Counter {
    fn next(
        &mut self,
        taken: &dyn Fn(&ElementId) -> bool,
    ) -> Result<ElementId, silico_core::Error> {
        loop {
            self.0 += 1;
            let id = ElementId::new(&format!("t{:07}", self.0)).unwrap();
            if !taken(&id) {
                return Ok(id);
            }
        }
    }
}

/// IDs `00000001`, `00000002`, ... in turn, skipping any taken: they sort before every ID
/// the tools draw from [`Counter`] (`t...`), so a dimension made with them is the first
/// constraint of the sketch's order, and PlaneGCS's lowest tag.
struct First(u32);

impl NewIds for First {
    fn next(
        &mut self,
        taken: &dyn Fn(&ElementId) -> bool,
    ) -> Result<ElementId, silico_core::Error> {
        loop {
            self.0 += 1;
            let id = ElementId::new(&format!("{:08}", self.0)).unwrap();
            if !taken(&id) {
                return Ok(id);
            }
        }
    }
}

/// Sessions on a fake kernel, each sketch solved by the sketch worker, with the empty
/// sketch open as s1; and a tool drawing on it.
struct Fixture {
    _dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    tool: SketchTool,
    ids: Counter,
    /// IDs that sort first, which Smart Dimension draws from while `sort_first` is set.
    first: First,
    sort_first: bool,
    keys: Modifiers,
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn sketch_id() -> FeatureId {
    FeatureId::new("s0000001").unwrap()
}

/// An empty sketch on the xy plane, the document's only feature, open as s1, and a tool
/// drawing on it.
fn opened(kind: ToolKind) -> Fixture {
    opened_with(kind, &sketch_doc("", "", ""))
}

/// The document `text` open as s1, and a tool drawing on its sketch `s0000001`.
fn opened_with(kind: ToolKind, text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &Path| Cache::unsealed(cache),
        || {
            SketchWorker::start_worker(Path::new(WORKER))
                .map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
        },
    );
    let path = dir.path().join("sketch.silico");
    std::fs::write(&path, text).unwrap();
    let opened = match sessions.call(
        "open",
        &json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
    ) {
        Call::Running(calling) => calling
            .wait_timeout(HANG)
            .unwrap_or_else(|_| panic!("open didn't answer in {HANG:?}")),
        Call::Unknown => panic!("no open"),
    };
    assert!(opened.ok, "{}", opened.text);
    Fixture {
        _dir: dir,
        sessions,
        tool: SketchTool::new(kind),
        ids: Counter(0),
        first: First(0),
        sort_first: false,
        keys: Modifiers::default(),
    }
}

impl Fixture {
    /// The current model's `sketch` read, once its rebuild has solved it.
    fn read(&self) -> SketchRead {
        self.read_at().0
    }

    /// The current model's `sketch` read, and the revision of the model it read.
    fn read_at(&self) -> (SketchRead, Revision) {
        let answer = self
            .sessions
            .sketch(&s1(), ModelChoice::Current, sketch_id())
            .wait_timeout(HANG)
            .unwrap_or_else(|_| panic!("sketch didn't answer in {HANG:?}"));
        let revision = answer.model.as_ref().unwrap().revision;
        (answer.result.unwrap_or_else(|p| panic!("{p:?}")), revision)
    }

    /// `event` sent to the tool against the newest read; what it adds sent as a
    /// `sketch_add`, which must be taken.
    fn send(&mut self, event: Event) {
        let (read, revision) = self.read_at();
        let sketch = sketch_id();
        let context = Context {
            sketch_id: &sketch,
            revision,
            sketch: &read.members,
            tolerance: TOLERANCE,
            automatic_relations: true,
            construction: false,
            states: &read.states,
        };
        let outcome = self.tool.handle(&context, &mut self.ids, event, self.keys);
        assert_eq!(outcome.message, None, "{event:?}");
        if let (Some(members), Some(token)) = (outcome.add, outcome.token) {
            let made = self.sketch_add(&members);
            self.tool.added(token, made);
        }
    }

    /// `members` sent as a `sketch_add`, which must be taken: the revision it made.
    fn sketch_add(&self, members: &Members) -> Revision {
        let revision = self.sessions.status(&s1()).session.unwrap().revision;
        let added = self
            .sessions
            .sketch_add(
                &s1(),
                revision,
                sketch_id(),
                NewMembers::of(members).unwrap(),
            )
            .wait_timeout(HANG)
            .unwrap_or_else(|_| panic!("sketch_add didn't answer in {HANG:?}"));
        let made = added.session.as_ref().unwrap().revision;
        let added = added.result.unwrap_or_else(|p| panic!("{p:?}"));
        assert!(added.changed);
        made
    }

    fn click(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(Event::Move([x, y]));
        self.send(Event::Press([x, y]));
        self.send(Event::Release([x, y]));
        self
    }

    fn drag(&mut self, from: [f64; 2], to: [f64; 2]) -> &mut Self {
        self.send(Event::Press(from));
        self.send(Event::Move(to));
        self.send(Event::Release(to));
        self
    }
}

/// Each point of the read, where the solve put it, sorted: to the nanometre.
fn points(read: &SketchRead) -> Vec<(i64, i64)> {
    let nm = |v: f64| (v * 1e6).round() as i64;
    let mut points: Vec<(i64, i64)> = read
        .members
        .points
        .iter()
        .map(|p| (nm(p.x.mm()), nm(p.y.mm())))
        .collect();
    points.sort();
    points
}

/// `(x, y)` in millimetres, as [`points`] gives them.
fn at(x: f64, y: f64) -> (i64, i64) {
    ((x * 1e6).round() as i64, (y * 1e6).round() as i64)
}

fn sorted(mut points: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    points.sort();
    points
}

/// Each constraint's state, by type, sorted.
fn constraint_states(read: &SketchRead) -> Vec<(&'static str, ConstraintState)> {
    let mut states: Vec<_> = read
        .members
        .constraints
        .iter()
        .map(|c| (c.form.type_name(), read.states.constraints[&c.id]))
        .collect();
    states.sort_by_key(|(name, state)| (*name, state.as_str()));
    states
}

#[test]
fn a_corner_rectangle_from_the_origin_solves_with_its_width_and_height_free() {
    // Four corners, eight unknowns; four sides' horizontal and vertical, and the first
    // corner level with the origin and above it: six equations, so two freedoms left, the
    // width and the height. One region of four lines: the corners are shared.
    let mut f = opened(ToolKind::CornerRectangle);
    f.drag([0.3, 0.2], [40.0, 20.0]);
    let read = f.read();
    assert_eq!(read.states.state, SketchState::UnderDefined);
    assert_eq!(read.states.dofs, 2);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(0.0, 0.0),
            at(40.0, 0.0),
            at(40.0, 20.0),
            at(0.0, 20.0)
        ])
    );
    assert_eq!(read.regions.len(), 1);
    assert_eq!(read.regions[0].outer.len(), 4);
    assert!(read.regions[0].holes.is_empty());
    assert!(
        read.states
            .constraints
            .values()
            .all(|s| *s == ConstraintState::Satisfied)
    );
}

#[test]
fn a_centre_rectangle_about_the_origin_solves_with_two_freedoms() {
    // Five points (the construction centre among them), ten unknowns; four sides' horizontal
    // and vertical (four), the corners symmetric about the centre (two), the centre at the
    // origin (two): eight, so two left. The corners ±(20, 10), the centre left out of the
    // profile: one region of four lines.
    let mut f = opened(ToolKind::CenterRectangle);
    f.drag([0.1, -0.2], [20.0, 10.0]);
    let read = f.read();
    assert_eq!(read.states.dofs, 2);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(0.0, 0.0),
            at(-20.0, -10.0),
            at(20.0, -10.0),
            at(20.0, 10.0),
            at(-20.0, 10.0),
        ])
    );
    assert_eq!(read.regions.len(), 1);
    assert_eq!(read.regions[0].outer.len(), 4);
}

#[test]
fn a_closed_chain_solves_as_its_horizontals_and_verticals_hold_it() {
    // Clicks (10, 10), (40, 10.3), (40.2, 30), (10.3, 30.2), and back at (10.1, 10.2): the
    // second is 0.57° off horizontal, moved to (40, 10); the third 0.57° off vertical, to
    // (40, 30); the fourth 0.39° off horizontal, to (10.3, 30), then onto x = 10 by the guide
    // from (10, 10), (10, 30); the last on the start, shared, its segment vertical. Four
    // points, eight unknowns, four relations: four freedoms (where it is, and its size).
    let mut f = opened(ToolKind::Line);
    f.click(10.0, 10.0)
        .click(40.0, 10.3)
        .click(40.2, 30.0)
        .click(10.3, 30.2)
        .click(10.1, 10.2);
    let read = f.read();
    assert_eq!(read.states.dofs, 4);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(10.0, 10.0),
            at(40.0, 10.0),
            at(40.0, 30.0),
            at(10.0, 30.0)
        ])
    );
    assert_eq!(
        constraint_states(&read),
        [
            ("horizontal", ConstraintState::Satisfied),
            ("horizontal", ConstraintState::Satisfied),
            ("vertical", ConstraintState::Satisfied),
            ("vertical", ConstraintState::Satisfied),
        ]
    );
    assert_eq!(read.regions.len(), 1);
    assert_eq!(read.regions[0].outer.len(), 4);
}

#[test]
fn a_horizontal_both_ends_on_the_axis_fix_is_left_out_by_the_tool_and_read_redundant_written_by_hand()
 {
    // #278's G as amended (Fable, 2026-11-08, g5-redundant-autoconstraints-fable.md): a line
    // with both ends on the x axis, the line between them level by the two `on`s. The tool
    // leaves its `horizontal` out: two constraints, none redundant, two freedoms (four
    // unknowns, each end's y fixed).
    let mut f = opened(ToolKind::Line);
    f.drag([10.0, 0.1], [40.0, -0.1]);
    let read = f.read();
    assert_eq!(read.states.dofs, 2);
    assert_eq!(read.states.state, SketchState::UnderDefined);
    assert_eq!(read.members.constraints.len(), 2);
    assert!(
        read.states
            .constraints
            .values()
            .all(|s| *s == ConstraintState::Satisfied),
        "{:?}",
        read.states.constraints
    );
    // The same sketch with the `horizontal` written by hand, as an agent or a person may: one
    // of the three relations repeats the others, read redundant, the freedoms the same two,
    // the sketch's state by them, under defined (M5.0's item 32, Addendum 2).
    let line = read.members.lines[0].id.clone();
    f.sketch_add(&Members {
        constraints: vec![silico_core::document::SketchConstraint {
            id: ElementId::new("k0000001").unwrap(),
            form: Form::Horizontal {
                line: Reference::Element(line),
            },
        }],
        ..Members::default()
    });
    let read = f.read();
    assert_eq!(read.states.dofs, 2);
    assert_eq!(read.states.state, SketchState::UnderDefined);
    assert_eq!(read.members.constraints.len(), 3);
    let redundant = read
        .states
        .constraints
        .values()
        .filter(|s| **s == ConstraintState::Redundant)
        .count();
    assert_eq!(redundant, 1, "{:?}", read.states.constraints);
}

#[test]
fn a_tangent_arc_off_a_horizontal_line_solves_where_it_was_drawn() {
    // The line (10, 10)-(30, 10), horizontal: four unknowns, one equation. The tangent arc
    // dragged from (30, 10) up to (30, 30): its centre (30, 20). The arc adds its centre and
    // its end, four unknowns, and its own rules hold its ends on it (its start, shared, one
    // equation for the end's radius, in the points' terms), and `tangent_at` one more: two
    // freedoms (radius, sweep). 3 + 2 = 5.
    let mut f = opened(ToolKind::Line);
    f.drag([10.0, 10.0], [30.0, 10.1]);
    f.tool = SketchTool::new(ToolKind::TangentArc);
    f.drag([30.0, 10.0], [30.0, 30.0]);
    let read = f.read();
    assert_eq!(read.states.dofs, 5);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(10.0, 10.0),
            at(30.0, 10.0),
            at(30.0, 20.0),
            at(30.0, 30.0)
        ])
    );
    assert_eq!(
        constraint_states(&read),
        [
            ("horizontal", ConstraintState::Satisfied),
            ("tangent_at", ConstraintState::Satisfied),
        ]
    );
}

#[test]
fn a_point_at_a_lines_middle_and_a_circle_at_the_origin_solve_as_counted() {
    // The line (10, 10)-(31, 25): four freedoms. A point clicked 0.1 mm from its middle
    // (20.5, 17.5): two unknowns, `midpoint` two equations, still four. A circle centred at
    // the origin with its radius to (6, 8): three unknowns, the origin's two relations: one
    // more freedom, its radius 10. 4 + 1 = 5.
    let mut f = opened(ToolKind::Line);
    f.drag([10.0, 10.0], [31.0, 25.0]);
    f.tool = SketchTool::new(ToolKind::Point);
    f.click(20.6, 17.5);
    assert_eq!(f.read().states.dofs, 4);
    f.tool = SketchTool::new(ToolKind::Circle);
    f.drag([0.2, 0.1], [6.0, 8.0]);
    let read = f.read();
    assert_eq!(read.states.dofs, 5);
    assert_eq!(read.members.circles.len(), 1);
    assert_eq!(read.members.circles[0].radius.mm(), 10.0);
    assert!(points(&read).contains(&at(20.5, 17.5)));
    assert!(points(&read).contains(&at(0.0, 0.0)));
}

#[test]
fn with_ctrl_held_a_chain_still_shares_its_joins_and_its_profile_closes() {
    // #278's F on the real core: a triangle clicked with Ctrl held, (10, 10), (40, 10.3),
    // (25, 30), and back on its start: three points shared by three lines, one region, no
    // relation, six freedoms.
    let mut f = opened(ToolKind::Line);
    f.keys.ctrl = true;
    f.click(10.0, 10.0)
        .click(40.0, 10.3)
        .click(25.0, 30.0)
        .click(10.2, 10.1);
    let read = f.read();
    assert!(read.members.constraints.is_empty());
    assert_eq!(read.members.points.len(), 3);
    assert_eq!(read.states.dofs, 6);
    assert_eq!(read.regions.len(), 1);
    assert_eq!(read.regions[0].outer.len(), 3);
}

#[test]
fn an_obround_drawn_with_tangent_arcs_holds_its_tangencies() {
    // Review item 1. The bottom (10, 10)-(40, 10), horizontal; a tangent arc up to (40, 30),
    // about (40, 20); the top from there back to (10, 30.2), leaving the arc along −x, so
    // tangent and horizontal, to (10, 30); a tangent arc down to the bottom's start, about
    // (10, 20). Six points, twelve unknowns; each arc's end held on its circle (one each in
    // the points' terms), two horizontals, three tangencies: 12 − 2 − 2 − 3 = 5 freedoms,
    // the fifth the last join, which no tool made tangent. Every constraint satisfied, the
    // points where drawn, one region of four curves.
    let mut f = opened(ToolKind::Line);
    f.drag([10.0, 10.0], [40.0, 10.1]);
    f.tool = SketchTool::new(ToolKind::TangentArc);
    f.drag([40.0, 10.0], [40.0, 30.0]);
    f.tool = SketchTool::new(ToolKind::Line);
    f.drag([40.0, 30.0], [10.0, 30.2]);
    f.tool = SketchTool::new(ToolKind::TangentArc);
    f.drag([10.0, 30.0], [10.1, 10.1]);
    let read = f.read();
    assert_eq!(read.states.dofs, 5);
    assert_eq!(
        constraint_states(&read),
        [
            ("horizontal", ConstraintState::Satisfied),
            ("horizontal", ConstraintState::Satisfied),
            ("tangent_at", ConstraintState::Satisfied),
            ("tangent_at", ConstraintState::Satisfied),
            ("tangent_at", ConstraintState::Satisfied),
        ]
    );
    assert_eq!(
        points(&read),
        sorted(vec![
            at(10.0, 10.0),
            at(40.0, 10.0),
            at(40.0, 20.0),
            at(40.0, 30.0),
            at(10.0, 30.0),
            at(10.0, 20.0),
        ])
    );
    assert_eq!(read.regions.len(), 1);
    assert_eq!(read.regions[0].outer.len(), 4);
}

#[test]
fn a_three_point_and_a_centrepoint_arc_solve_where_they_were_drawn() {
    // Review item 13. A 3-point arc from (10, 10) to (30, 10) through (20, 20): about
    // (20, 10), radius 10. Three free points, six unknowns, the end held on the circle: five
    // freedoms. A centrepoint arc about (60, 50) from (68, 56) swept counter-clockwise to
    // (54, 58) (radius 10, a 6-8-10 each way): five more. Each point where it was drawn.
    let mut f = opened(ToolKind::ThreePointArc);
    f.click(10.0, 10.0).click(30.0, 10.0).click(20.0, 20.0);
    assert_eq!(f.read().states.dofs, 5);
    f.tool = SketchTool::new(ToolKind::CenterpointArc);
    f.click(60.0, 50.0).click(68.0, 56.0);
    f.send(Event::Move([63.0, 60.0]));
    f.click(54.0, 58.0);
    let read = f.read();
    assert_eq!(read.states.dofs, 10);
    assert_eq!(read.members.arcs.len(), 2);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(20.0, 10.0),
            at(10.0, 10.0),
            at(30.0, 10.0),
            at(60.0, 50.0),
            at(68.0, 56.0),
            at(54.0, 58.0),
        ])
    );
}

#[test]
fn a_line_leaving_an_arc_a_degree_off_an_axis_solves_tangent_with_nothing_in_conflict() {
    // Round 2's rule: the arc about (50, 50), radius 10, from 270° to 91°, leaves its end
    // along 181°, 1° off −x. A line drawn from there toward 180.5° gets `tangent_at` alone,
    // written along 181°; were it horizontal too, the two would conflict. The arc (centre,
    // start, end: six unknowns, its end held on it) has five freedoms, the line's far end
    // two more, the tangency takes one: six. Nothing conflicts; the line's end solves where
    // it was drawn.
    let deg = |d: f64| d.to_radians();
    let end = [
        50.0 + 10.0 * libm::cos(deg(91.0)),
        50.0 + 10.0 * libm::sin(deg(91.0)),
    ];
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{{"id": "c0000001", "x": 50, "y": 50, "construction": false}}, {{"id": "q0000101", "x": 50, "y": 40, "construction": false}}, {{"id": "q0000102", "x": {}, "y": {}, "construction": false}}], "lines": [], "circles": [], "arcs": [{{"id": "a0000001", "center": "c0000001", "start": "q0000101", "end": "q0000102", "construction": false}}], "constraints": []}}]}}"#,
        end[0], end[1]
    );
    let mut f = opened_with(ToolKind::Line, &text);
    let toward = [
        end[0] + 30.0 * libm::cos(deg(180.5)),
        end[1] + 30.0 * libm::sin(deg(180.5)),
    ];
    f.drag(end, toward);
    let read = f.read();
    assert_eq!(read.states.dofs, 6);
    assert_eq!(read.states.state, SketchState::UnderDefined);
    assert_eq!(
        constraint_states(&read),
        [("tangent_at", ConstraintState::Satisfied)]
    );
    let along = 30.0 * libm::cos(deg(0.5));
    let want = at(
        end[0] + along * libm::cos(deg(181.0)),
        end[1] + along * libm::sin(deg(181.0)),
    );
    assert!(
        points(&read).contains(&want),
        "{:?} {want:?}",
        points(&read)
    );
}

// --- Smart Dimension: SD7 on the real solve --------------------------------------------

impl Fixture {
    /// `event` sent to Smart Dimension against the newest read; what it adds sent as a
    /// `sketch_add`, which must be taken. Nothing said.
    fn dimension(&mut self, tool: &mut SmartDimension, event: DimensionEvent) {
        let (read, revision) = self.read_at();
        let sketch = sketch_id();
        let context = Context {
            sketch_id: &sketch,
            revision,
            sketch: &read.members,
            tolerance: TOLERANCE,
            automatic_relations: true,
            construction: false,
            states: &read.states,
        };
        let ids: &mut dyn NewIds = if self.sort_first {
            &mut self.first
        } else {
            &mut self.ids
        };
        let outcome = tool.handle(&context, ids, event.clone());
        assert_eq!(outcome.message, None, "{event:?}");
        if let (Some(members), Some(token)) = (outcome.add, outcome.token) {
            let made = self.sketch_add(&members);
            tool.added(token, made);
        }
    }

    fn pick(&mut self, tool: &mut SmartDimension, x: f64, y: f64) -> &mut Self {
        self.dimension(tool, DimensionEvent::Move([x, y]));
        self.dimension(tool, DimensionEvent::Press([x, y]));
        self.dimension(tool, DimensionEvent::Release([x, y]));
        self
    }

    fn text(&self) -> String {
        self.sessions
            .text(&s1(), None)
            .wait_timeout(HANG)
            .unwrap_or_else(|_| panic!("text didn't answer in {HANG:?}"))
            .result
            .unwrap()
            .text
    }

    /// The offer for the dimension `tool` last placed, against the newest read.
    fn offer(&self, tool: &SmartDimension) -> Option<Offer> {
        let (read, revision) = self.read_at();
        offer(&read, revision, &tool.placement().unwrap())
    }
}

/// The corner rectangle from the origin to (40, 20), in a transaction, its top set to 50
/// and its right side's 20 kept: two dimensions, fully defined. Its corners then (0, 0),
/// (50, 0), (50, 20), (0, 20).
fn dimensioned() -> (Fixture, SmartDimension) {
    let mut f = opened(ToolKind::CornerRectangle);
    f.sessions
        .begin(&s1())
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("begin didn't answer in {HANG:?}"))
        .result
        .unwrap();
    f.drag([0.3, 0.2], [40.0, 20.0]);
    assert_eq!(f.read().states.dofs, 2);
    let mut tool = SmartDimension::new();
    // The top, picked at (20, 20), placed above it; the box's 40 replaced by 50.
    f.pick(&mut tool, 20.0, 20.0).pick(&mut tool, 20.0, 26.0);
    assert_eq!(tool.preview().modify.as_deref(), Some("40"));
    f.dimension(&mut tool, DimensionEvent::Typed("50".to_owned()));
    f.dimension(&mut tool, DimensionEvent::Enter);
    assert_eq!(f.read().states.dofs, 1);
    // The right side, now at x = 50, picked at (50, 10), placed beside it; the box skipped:
    // its measured 20.
    f.pick(&mut tool, 50.0, 10.0).pick(&mut tool, 56.0, 10.0);
    assert_eq!(tool.preview().modify.as_deref(), Some("20"));
    f.dimension(&mut tool, DimensionEvent::Escape);
    let read = f.read();
    assert_eq!(read.states.state, SketchState::FullyDefined);
    assert_eq!(read.states.dofs, 0);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(0.0, 0.0),
            at(50.0, 0.0),
            at(50.0, 20.0),
            at(0.0, 20.0)
        ])
    );
    assert!(f.offer(&tool).is_none());
    (f, tool)
}

/// The top-left corner (0, 20) to the bottom-right (50, 0), horizontally, placed above the
/// rectangle, with `value` typed: x(b) − x(a), 50 as drawn. Its ID sorts before every other
/// member's, so it's PlaneGCS's lowest tag, and never the one its contest flags on a tie.
fn third(f: &mut Fixture, tool: &mut SmartDimension, value: &str) {
    f.sort_first = true;
    f.pick(tool, 0.0, 20.0)
        .pick(tool, 50.0, 0.0)
        .pick(tool, 25.0, 30.0);
    assert_eq!(tool.preview().modify.as_deref(), Some("50"));
    f.dimension(tool, DimensionEvent::Typed(value.to_owned()));
    f.dimension(tool, DimensionEvent::Enter);
}

#[test]
fn a_conflicting_dimension_is_placed_read_over_defined_and_driven_solves_it() {
    // SD7, M5.11's test: the rectangle fully defined, a third dimension of 60 where the
    // geometry makes it 50 is placed, not refused; the sketch reads over defined, naming
    // it conflicting; the offer is SolidWorks' three; driven sets its `driving` to false,
    // and the sketch solves, fully defined again, the reference dimension measuring 50.
    let (mut f, mut tool) = dimensioned();
    third(&mut f, &mut tool, "60");
    let k = tool.placement().unwrap().constraint;
    let read = f.read();
    assert_eq!(read.members.constraints[0].id, k, "sorted first");
    assert_eq!(read.states.state, SketchState::OverDefined);
    assert_eq!(read.states.constraints[&k], ConstraintState::Conflicting);
    let offer = f.offer(&tool).unwrap();
    assert_eq!(offer.constraint, k);
    assert_eq!(offer.state, ConstraintState::Conflicting);
    assert_eq!(offer.request(OfferChoice::Leave), None);
    let Some(OfferRequest::SetField {
        revision,
        feature,
        field,
        value,
    }) = offer.request(OfferChoice::Driven)
    else {
        panic!("{offer:?}");
    };
    let set = f
        .sessions
        .set_field(&s1(), revision, feature, field, value)
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("set_field didn't answer in {HANG:?}"));
    assert!(set.result.unwrap().changed);
    let read = f.read();
    assert_eq!(read.states.state, SketchState::FullyDefined);
    assert_eq!(read.states.dofs, 0);
    assert_eq!(read.states.constraints[&k], ConstraintState::Satisfied);
    let placed = read.members.constraints.iter().find(|c| c.id == k).unwrap();
    assert_eq!(
        placed.form,
        Form::DistanceX {
            a: Reference::Element(top_left(&read)),
            b: Reference::Element(bottom_right(&read)),
            value: Length::parse("50").unwrap(),
            driving: false,
        }
    );
}

#[test]
fn cancel_undoes_a_conflicting_dimension_to_the_text_before_it_byte_for_byte() {
    let (mut f, mut tool) = dimensioned();
    let before = f.text();
    third(&mut f, &mut tool, "60");
    assert_ne!(f.text(), before);
    let offer = f.offer(&tool).unwrap();
    let Some(OfferRequest::Undo { revision, to }) = offer.request(OfferChoice::Cancel) else {
        panic!("{offer:?}");
    };
    f.sessions
        .undo(&s1(), revision, to)
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("undo didn't answer in {HANG:?}"))
        .result
        .unwrap();
    assert_eq!(f.text(), before);
    assert_eq!(f.read().states.state, SketchState::FullyDefined);
}

#[test]
fn an_agreeing_third_dimension_is_offered_whichever_of_its_group_planegcs_flags() {
    // 50, as drawn: redundant, not conflicting. Its ID sorts first, so PlaneGCS's contest
    // (popularity, fewer rows, then the later tag) flags another of the group, the top's
    // length, the latest tag: a driving dimension that repeats others, so the sketch is over
    // defined (M5.0 item 32). The offer follows the sketch's state, not the flag.
    let (mut f, mut tool) = dimensioned();
    third(&mut f, &mut tool, "50");
    let read = f.read();
    let k = tool.placement().unwrap().constraint;
    assert_eq!(read.members.constraints[0].id, k);
    assert_eq!(read.states.state, SketchState::OverDefined);
    assert_eq!(read.states.constraints[&k], ConstraintState::Satisfied);
    let flagged: Vec<&'static str> = read
        .members
        .constraints
        .iter()
        .filter(|c| read.states.constraints[&c.id] == ConstraintState::Redundant)
        .map(|c| c.form.type_name())
        .collect();
    assert_eq!(flagged, ["length"], "{:?}", read.states.constraints);
    let offer = f.offer(&tool).unwrap();
    assert_eq!(offer.constraint, k);
    assert_eq!(offer.sketch_state, SketchState::OverDefined);
}

#[test]
fn a_dimension_that_leaves_no_solution_is_offered_and_driven_solves_it() {
    // A triangle clicked with Ctrl held (no relations): (10, 10), (20, 10), (15, 18.660254),
    // back on its start. Its sides 10 and 10 dimensioned (the boxes skipped: (15, 18.660254)
    // is 10 from each to the nanometre), it's under defined: six unknowns, two equations,
    // four freedoms (where it is, its turn, and the angle at (10, 10)). The third side typed 30: no triangle has sides 10, 10, and 30, so no
    // solution, from a state that wasn't: offered. Driven, it solves, the third measuring
    // 10 again.
    let mut f = opened(ToolKind::Line);
    f.keys.ctrl = true;
    let apex = [15.0, 18.660254];
    f.click(10.0, 10.0)
        .click(20.0, 10.0)
        .click(apex[0], apex[1])
        .click(10.1, 10.0);
    let mut tool = SmartDimension::new();
    let left = [12.5, (10.0 + apex[1]) / 2.0];
    let right = [17.5, left[1]];
    f.pick(&mut tool, 15.0, 10.0).pick(&mut tool, 15.0, 5.0);
    assert_eq!(tool.preview().modify.as_deref(), Some("10"));
    f.dimension(&mut tool, DimensionEvent::Escape);
    f.pick(&mut tool, left[0], left[1])
        .pick(&mut tool, 5.0, 20.0);
    assert_eq!(tool.preview().modify.as_deref(), Some("10"));
    f.dimension(&mut tool, DimensionEvent::Escape);
    let read = f.read();
    assert_eq!(read.states.state, SketchState::UnderDefined);
    assert_eq!(read.states.dofs, 4);
    f.pick(&mut tool, right[0], right[1])
        .pick(&mut tool, 30.0, 20.0);
    f.dimension(&mut tool, DimensionEvent::Typed("30".to_owned()));
    f.dimension(&mut tool, DimensionEvent::Enter);
    let read = f.read();
    assert_eq!(read.states.state, SketchState::NoSolution);
    let offer = f.offer(&tool).unwrap();
    assert_eq!(offer.sketch_state, SketchState::NoSolution);
    assert_eq!(offer.constraint, tool.placement().unwrap().constraint);
    let Some(OfferRequest::SetField {
        revision,
        feature,
        field,
        value,
    }) = offer.request(OfferChoice::Driven)
    else {
        panic!("{offer:?}");
    };
    f.sessions
        .set_field(&s1(), revision, feature, field, value)
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("set_field didn't answer in {HANG:?}"))
        .result
        .unwrap();
    let read = f.read();
    assert_eq!(read.states.state, SketchState::UnderDefined);
    let k = offer.constraint;
    let placed = read.members.constraints.iter().find(|c| c.id == k).unwrap();
    assert!(
        matches!(placed.form, Form::Length { value, driving: false, .. } if value == Length::parse("10").unwrap()),
        "{placed:?}"
    );
}

#[test]
fn each_equal_horizontal_distance_after_the_first_is_offered() {
    // Round 3, item 1. Two points, (10, 10) and (40, 30): four freedoms. The first
    // `distance_x` from one to the other, 30, takes one: three. Each later one, 30 again,
    // adds an equation PlaneGCS finds dependent, so the freedoms stay three, and each is
    // offered: the fourth too, though PlaneGCS's greedy contest, a constraint flagged
    // already covering its group, may flag nothing new for it. Each answered Leave.
    let mut f = opened(ToolKind::Point);
    f.click(10.0, 10.0).click(40.0, 30.0);
    assert_eq!(f.read().states.dofs, 4);
    let mut tool = SmartDimension::new();
    for n in 0..4 {
        f.pick(&mut tool, 10.0, 10.0)
            .pick(&mut tool, 40.0, 30.0)
            .pick(&mut tool, 25.0, 45.0);
        assert_eq!(tool.preview().modify.as_deref(), Some("30"), "{n}");
        f.dimension(&mut tool, DimensionEvent::Escape);
        let (read, revision) = f.read_at();
        assert_eq!(read.states.dofs, 3, "{n}");
        let offered = f.offer(&tool);
        assert_eq!(offered.is_some(), n > 0, "{n}: {:?}", read.states);
        // The tool given the same read holds for the same offer.
        let sketch = sketch_id();
        let context = Context {
            sketch_id: &sketch,
            revision,
            sketch: &read.members,
            tolerance: TOLERANCE,
            automatic_relations: true,
            construction: false,
            states: &read.states,
        };
        tool.settle(&context);
        assert_eq!(tool.busy(), n > 0, "{n}");
        assert_eq!(tool.open_offer().is_some(), n > 0, "{n}");
        tool.dismiss_offer();
    }
}

#[test]
fn a_chain_corner_typed_60_solves_to_60() {
    // Fable's chain-angle decision, §3, on PlaneGCS. A chain clicked with Ctrl held (no
    // relations): A (10, 10), B (30, 10), C (21, 24), ended with Enter; a from A to B, b from
    // B to C. Dimensioned with the boxes skipped but two: A's x and y from the origin, 10
    // and 10; B's y, 10; a's length, 20; b's length typed 20; and the corner at B, the
    // pointer inside it (150° round from B), typed 60. Six unknowns, six equations.
    // The corner's along-rays leave B: a's back along a, 180°; b's along b. Counter-clockwise
    // b's then a's, a's against its line: `{b, a, reversed}`, 60, so b leaves B at
    // 180 − 60 = 120°, and C = B + 20 (cos 120°, sin 120°) = (20, 10 + 10√3), 27.320508 to
    // the nanometre. (Without `reversed`, as SD5 wrote it, 60 would hold b's direction 60°
    // short of a's, 300°, putting C below a.)
    let mut f = opened(ToolKind::Line);
    f.keys.ctrl = true;
    f.click(10.0, 10.0).click(30.0, 10.0).click(21.0, 24.0);
    f.send(Event::Enter);
    assert_eq!(f.read().states.dofs, 6);
    let mut tool = SmartDimension::new();
    let skipped = |f: &mut Fixture, tool: &mut SmartDimension, picks: &[[f64; 2]], at: [f64; 2]| {
        for p in picks {
            f.pick(tool, p[0], p[1]);
        }
        f.pick(tool, at[0], at[1]);
        f.dimension(tool, DimensionEvent::Escape);
    };
    let typed = |f: &mut Fixture,
                 tool: &mut SmartDimension,
                 picks: &[[f64; 2]],
                 at: [f64; 2],
                 value: &str| {
        for p in picks {
            f.pick(tool, p[0], p[1]);
        }
        f.pick(tool, at[0], at[1]);
        f.dimension(tool, DimensionEvent::Typed(value.to_owned()));
        f.dimension(tool, DimensionEvent::Enter);
    };
    skipped(&mut f, &mut tool, &[[0.1, 0.1], [10.0, 10.0]], [5.0, 20.0]);
    skipped(&mut f, &mut tool, &[[0.1, 0.1], [10.0, 10.0]], [20.0, 5.0]);
    skipped(&mut f, &mut tool, &[[0.1, 0.1], [30.0, 10.0]], [35.0, 5.0]);
    skipped(&mut f, &mut tool, &[[20.0, 10.0]], [20.0, 5.0]);
    assert_eq!(f.read().states.dofs, 2);
    typed(&mut f, &mut tool, &[[27.3, 14.2]], [35.0, 20.0], "20");
    // b, 20 long now, leaves B (30, 10) as drawn, about 122.7°: picked 5 along it.
    typed(
        &mut f,
        &mut tool,
        &[[20.0, 10.0], [27.3, 14.2]],
        [25.67, 12.5],
        "60",
    );
    let read = f.read();
    assert_eq!(read.states.dofs, 0);
    assert_eq!(read.states.state, SketchState::FullyDefined);
    assert_eq!(
        points(&read),
        sorted(vec![
            at(10.0, 10.0),
            at(30.0, 10.0),
            at(20.0, 10.0 + 10.0 * 3f64.sqrt())
        ])
    );
    let k = tool.placement().unwrap().constraint;
    let placed = read.members.constraints.iter().find(|c| c.id == k).unwrap();
    assert!(
        matches!(
            &placed.form,
            Form::Angle { reversed: true, value, driving: true, .. }
                if *value == silico_core::document::Angle::parse("60").unwrap()
        ),
        "{placed:?}"
    );
}

/// The read's point at (0, 20), and at (50, 0).
fn top_left(read: &SketchRead) -> ElementId {
    point_at(read, at(0.0, 20.0))
}

fn bottom_right(read: &SketchRead) -> ElementId {
    point_at(read, at(50.0, 0.0))
}

fn point_at(read: &SketchRead, want: (i64, i64)) -> ElementId {
    let nm = |v: f64| (v * 1e6).round() as i64;
    read.members
        .points
        .iter()
        .find(|p| (nm(p.x.mm()), nm(p.y.mm())) == want)
        .unwrap()
        .id
        .clone()
}

// --- #278's G as amended: what a tool leaves out, against PlaneGCS ----------------------------

/// A small generator of its own (an LCG, Knuth's constants), so each run draws the same
/// sketches.
struct Draws(u64);

impl Draws {
    /// The next number in [0, 1).
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A whole number in [lo, hi].
    fn between(&mut self, lo: i32, hi: i32) -> f64 {
        f64::from(lo) + (self.next() * f64::from(hi - lo + 1)).floor()
    }
}

/// A chain the Line tool draws from a random start, then Enter: each click at the origin
/// now and then, on or near an axis, near level or plumb with the last (within a degree),
/// or anywhere, a few millimetres apart at least.
fn draw_chain(f: &mut Fixture, draws: &mut Draws) {
    let mut last = [draws.between(-40, 40), draws.between(-40, 40)];
    if draws.next() < 0.2 {
        last = [0.0, 0.0];
    }
    f.click(last[0], last[1]);
    for _ in 0..(3 + (draws.next() * 3.0) as usize) {
        let roll = draws.next();
        let step = draws.between(8, 30) * if draws.next() < 0.5 { -1.0 } else { 1.0 };
        let next = if roll < 0.25 {
            [last[0] + step, 0.1]
        } else if roll < 0.4 {
            [0.1, last[1] + step]
        } else if roll < 0.6 {
            [last[0] + step, last[1] + step * 0.01]
        } else if roll < 0.8 {
            [last[0] + step * 0.01, last[1] + step]
        } else {
            [last[0] + step, last[1] + draws.between(-30, 30)]
        };
        f.click(next[0], next[1]);
        last = next;
    }
    f.send(Event::Enter);
}

/// Each relation a tool can infer between `read`'s members, as an agent would write it:
/// each line `horizontal` and `vertical`; each pair of points level and plumb; each point
/// on each axis; each pair of lines sharing an end at right angles and parallel.
fn candidates(read: &SketchRead) -> Vec<Form> {
    let r = |id: &ElementId| Reference::Element(id.clone());
    let sketch = &read.members;
    let mut forms = Vec::new();
    for line in &sketch.lines {
        forms.push(Form::Horizontal { line: r(&line.id) });
        forms.push(Form::Vertical { line: r(&line.id) });
    }
    for (k, a) in sketch.points.iter().enumerate() {
        for curve in [Reference::XAxis, Reference::YAxis] {
            forms.push(Form::On {
                point: r(&a.id),
                curve,
            });
        }
        for b in &sketch.points[k + 1..] {
            forms.push(Form::HorizontalPoints {
                a: r(&a.id),
                b: r(&b.id),
            });
            forms.push(Form::VerticalPoints {
                a: r(&a.id),
                b: r(&b.id),
            });
        }
    }
    for (k, a) in sketch.lines.iter().enumerate() {
        for b in &sketch.lines[k + 1..] {
            let ends = [&a.from, &a.to];
            if ends.contains(&&b.from) || ends.contains(&&b.to) {
                forms.push(Form::Perpendicular {
                    a: r(&a.id),
                    b: r(&b.id),
                });
                forms.push(Form::Parallel {
                    a: r(&a.id),
                    b: r(&b.id),
                });
            }
        }
    }
    forms
}

/// Whether `form` is one of the equalities the implied classes read: a `horizontal`,
/// `vertical`, `horizontal_points`, `vertical_points`, or `on` of an axis.
fn is_read_equality(form: &Form) -> bool {
    matches!(
        form,
        Form::Horizontal { .. }
            | Form::Vertical { .. }
            | Form::HorizontalPoints { .. }
            | Form::VerticalPoints { .. }
            | Form::On {
                curve: Reference::XAxis | Reference::YAxis,
                ..
            }
    )
}

/// Whether a constraint read `state` is flagged as the rule expects of one it leaves out:
/// redundant, partially redundant, or conflicting.
fn flagged(state: Option<ConstraintState>) -> bool {
    matches!(
        state,
        Some(
            ConstraintState::Redundant
                | ConstraintState::PartiallyRedundant
                | ConstraintState::Conflicting
        )
    )
}

/// `form` added to the session inside a transaction, its read, then rolled back: the
/// freedoms it leaves, and its own state.
fn tried(f: &mut Fixture, form: &Form, n: &mut u32) -> (i32, Option<ConstraintState>) {
    f.sessions
        .begin(&s1())
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("begin didn't answer in {HANG:?}"))
        .result
        .unwrap();
    *n += 1;
    let id = ElementId::new(&format!("{:08}", 90_000_000 + *n)).unwrap();
    let members = Members {
        constraints: vec![silico_core::document::SketchConstraint {
            id: id.clone(),
            form: form.clone(),
        }],
        ..Members::default()
    };
    f.sketch_add(&members);
    let read = f.read();
    f.sessions
        .rollback(&s1())
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("rollback didn't answer in {HANG:?}"))
        .result
        .unwrap();
    (read.states.dofs, read.states.constraints.get(&id).copied())
}

#[test]
fn a_relation_the_tools_leave_out_is_one_planegcs_finds_redundant_or_conflicting() {
    // Twelve sketches the Line tool draws at random (two chains each), some points then
    // fixed by driving distances from the origin as drawn. Of every relation a tool could
    // infer between their members, each the rule leaves out (`implied_by`), added back and
    // solved on PlaneGCS, is read redundant, partially redundant, or conflicting, or leaves
    // the freedoms as they were: the rule never leaves out what the solver would keep. And on
    // a sketch holding only the forms the classes read (no dimensions, no `on` of a line,
    // no direction between lines), each `horizontal`, `vertical`, `horizontal_points`,
    // `vertical_points`, or `on` of an axis the rule keeps lowers the freedoms by one.
    use silico_core::sketch_tools::implied_by;
    let mut draws = Draws(0x0051_11c0);
    let (mut left_out, mut kept_tried) = (0, 0);
    let mut left_out_by_form = std::collections::BTreeMap::<&str, usize>::new();
    for sketch in 0..12 {
        let mut f = opened(ToolKind::Line);
        draw_chain(&mut f, &mut draws);
        draw_chain(&mut f, &mut draws);
        let mut read = f.read();
        // What the tools added (every constraint, the sketch begun empty): none of the
        // equalities the classes read is redundant or conflicting.
        for c in &read.members.constraints {
            let state = read.states.constraints.get(&c.id).copied();
            assert!(
                !is_read_equality(&c.form) || !flagged(state),
                "sketch {sketch}: the tools added {:?}, read {state:?}; the sketch: {:?}",
                c.form,
                read.members
            );
        }
        let equalities_only = read
            .members
            .constraints
            .iter()
            .all(|c| is_read_equality(&c.form));
        if sketch % 2 == 1 {
            // Some points fixed where they are: a driving distance x and y from the origin.
            let mut constraints = Vec::new();
            for (k, p) in read
                .members
                .points
                .iter()
                .enumerate()
                .filter(|(k, _)| k % 2 == 0)
            {
                for (axis, value) in [("x", p.x.mm()), ("y", p.y.mm())] {
                    let value = Length::rounded(value).unwrap();
                    let (a, b) = (Reference::Origin, Reference::Element(p.id.clone()));
                    let form = if axis == "x" {
                        Form::DistanceX {
                            a,
                            b,
                            value,
                            driving: true,
                        }
                    } else {
                        Form::DistanceY {
                            a,
                            b,
                            value,
                            driving: true,
                        }
                    };
                    constraints.push(silico_core::document::SketchConstraint {
                        id: ElementId::new(&format!(
                            "{:08}",
                            80_000_000 + 2 * k + usize::from(axis == "y")
                        ))
                        .unwrap(),
                        form,
                    });
                }
            }
            f.sketch_add(&Members {
                constraints,
                ..Members::default()
            });
            read = f.read();
        }
        let before = read.states.dofs;
        let mut n = 0;
        for form in candidates(&read) {
            if implied_by(&read.members, &read.states, &form) {
                left_out += 1;
                *left_out_by_form.entry(form.type_name()).or_default() += 1;
                let (dofs, state) = tried(&mut f, &form, &mut n);
                assert!(
                    dofs == before || flagged(state),
                    "sketch {sketch}: {form:?} left out, but added it leaves {dofs} freedoms of \
                     {before}, read {state:?}; the sketch: {:?}",
                    read.members
                );
            } else if sketch % 2 == 0
                && equalities_only
                && !matches!(form, Form::Perpendicular { .. } | Form::Parallel { .. })
            {
                kept_tried += 1;
                let (dofs, state) = tried(&mut f, &form, &mut n);
                assert_eq!(
                    dofs,
                    before - 1,
                    "sketch {sketch}: {form:?} kept, read {state:?}; the sketch: {:?}",
                    read.members
                );
            }
        }
    }
    // The draws reach both halves (each count worked by the run, so a generator that
    // stopped drawing axes or levels would show here).
    assert!(left_out >= 20, "{left_out} left out");
    assert!(kept_tried >= 20, "{kept_tried} kept tried");
    // Rule F's half too: a perpendicular or a parallel among those left out.
    let directions = ["perpendicular", "parallel"]
        .iter()
        .filter_map(|form| left_out_by_form.get(form))
        .sum::<usize>();
    assert!(directions > 0, "left out by form: {left_out_by_form:?}");
}

#[test]
fn a_line_between_two_fixed_points_off_level_solves_without_the_horizontal_the_tool_leaves_out() {
    // (10, 10) and (40, 10.785), each fixed by its distances from the origin: 1.499° apart,
    // within 2°. The tool's line between them sends no `horizontal`, and the sketch solves,
    // fully defined. The same line with `horizontal` added by hand conflicts: over defined.
    let points = r#"[{"id": "q0000001", "x": 10, "y": 10, "construction": false}, {"id": "q0000002", "x": 40, "y": 10.785, "construction": false}]"#;
    let dims = r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "q0000001", "value": 10, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "q0000001", "value": 10, "driving": true}, {"id": "k0000003", "type": "distance_x", "a": "origin", "b": "q0000002", "value": 40, "driving": true}, {"id": "k0000004", "type": "distance_y", "a": "origin", "b": "q0000002", "value": 10.785, "driving": true}"#;
    let text = |lines: &str, more: &str| {
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": {points}, "lines": [{lines}], "circles": [], "arcs": [], "constraints": [{dims}{more}]}}]}}"#
        )
    };
    let mut f = opened_with(ToolKind::Line, &text("", ""));
    assert_eq!(f.read().states.state, SketchState::FullyDefined);
    f.drag([10.0, 10.0], [40.0, 10.785]);
    let read = f.read();
    assert_eq!(read.members.lines.len(), 1);
    assert_eq!(read.members.constraints.len(), 4, "no horizontal");
    assert_eq!(read.states.state, SketchState::FullyDefined);
    let by_hand = opened_with(
        ToolKind::Line,
        &text(
            r#"{"id": "n0000001", "from": "q0000001", "to": "q0000002", "construction": false}"#,
            r#", {"id": "k0000005", "type": "horizontal", "line": "n0000001"}"#,
        ),
    );
    let read = by_hand.read();
    assert_eq!(
        read.states.state,
        SketchState::OverDefined,
        "{:?}",
        read.states
    );
}

// --- SD7's offer on the real solve, by the dimension's own row -----------------------------
//
// `offer`'s rule, c3 as amended (Fable, 2026-11-09, c3-refused-sketch-fable.md): an add is
// offered by the freedoms it left (its row dependent) or its own flag, an edit by any
// constraint it newly set in conflict, either by a solve newly refused. PlaneGCS's contest
// breaks its ties by the tags, the constraints' IDs in order, so each case below that turns on
// a tie runs with the change's ID first and last, and gives the same offer both ways.

/// A document of one sketch on xy: its `points`, `lines`, and `constraints`, each a JSON
/// list's insides.
fn sketch_doc(points: &str, lines: &str, constraints: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{points}], "lines": [{lines}], "circles": [], "arcs": [], "constraints": [{constraints}]}}]}}"#
    )
}

fn element(id: &str) -> ElementId {
    ElementId::new(id).unwrap()
}

/// The constraints `read` flags (conflicting, redundant, or partially), by ID.
fn flagged_in(read: &SketchRead) -> BTreeSet<ElementId> {
    read.states
        .constraints
        .iter()
        .filter(|(_, state)| state.flagged())
        .map(|(id, _)| id.clone())
        .collect()
}

/// The constraints `read` gives `state`, by ID, in order.
fn listed(read: &SketchRead, state: ConstraintState) -> Vec<&str> {
    read.states
        .constraints
        .iter()
        .filter(|(_, s)| **s == state)
        .map(|(id, _)| id.as_str())
        .collect()
}

/// The state `read` gives the constraint `id`.
fn state_of(read: &SketchRead, id: &str) -> ConstraintState {
    read.states.constraints[&element(id)]
}

/// The code `read`'s solve was refused with, if it was.
fn refusal(read: &SketchRead) -> Option<ErrorCode> {
    read.states
        .solve_problem
        .as_ref()
        .map(silico_core::Error::code)
}

/// A driving `length` of the line `line`, `value` mm.
fn length_of(line: &str, value: &str) -> Form {
    Form::Length {
        line: Reference::Element(element(line)),
        value: Length::parse(value).unwrap(),
        driving: true,
    }
}

/// A driving `length` of the line `line`, `value` mm, as the constraint `id`: a document's
/// constraint.
fn length_json(id: &str, line: &str, value: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "length", "line": "{line}", "value": {value}, "driving": true}}"#
    )
}

/// `form` added as the constraint `id` by `sketch_add`.
fn add_constraint(f: &Fixture, id: &str, form: Form) {
    f.sketch_add(&Members {
        constraints: vec![SketchConstraint {
            id: element(id),
            form,
        }],
        ..Members::default()
    });
}

/// The constraint `id`'s value set to `text`, as the app sends a placed dimension's edit: the
/// text read as the Modify box reads it, sent as a `set_field` of its `value`, which must
/// change the document.
fn set_value(f: &Fixture, id: &str, text: &str) {
    let read = f.read();
    let form = &read
        .members
        .constraints
        .iter()
        .find(|c| c.id == element(id))
        .unwrap()
        .form;
    let value = read_typed(form, text)
        .unwrap_or_else(|m| panic!("{m:?}"))
        .node();
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let set = f
        .sessions
        .set_field(
            &s1(),
            revision,
            sketch_id(),
            format!("constraints.{id}.value"),
            value,
        )
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("set_field didn't answer in {HANG:?}"));
    assert!(set.result.unwrap_or_else(|p| panic!("{p:?}")).changed);
}

/// The document replaced whole by `text`.
fn replace(f: &Fixture, text: String) {
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    f.sessions
        .replace_text(&s1(), revision, text)
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("replace_text didn't answer in {HANG:?}"))
        .result
        .unwrap_or_else(|p| panic!("{p:?}"));
}

/// Which change [`judged`] judges: a dimension added, or its value edited.
#[derive(Debug, Clone, Copy)]
enum Change {
    Add,
    Edit,
}

/// The placement of the change `change` of the constraint `id`, made on the read `before`
/// of the revision `at`, as the tool (an add) and the app (an edit) record it, its change
/// the revision `made`.
fn placement_of(
    before: &SketchRead,
    at: Revision,
    made: Revision,
    id: &str,
    change: Change,
) -> Placement {
    Placement {
        sketch: sketch_id(),
        constraint: element(id),
        revision: made,
        read: at,
        kind: match change {
            Change::Add => PlacementKind::add_on(&before.states),
            Change::Edit => PlacementKind::edit_on(&before.states),
        },
    }
}

/// What SD7 offers for the constraint `id` (`change`, an add or an edit) on the read after
/// the change, the read `before` of the revision `at` the one before it, nothing between
/// them: the core's own [`offer`].
fn judged(
    f: &Fixture,
    before: &SketchRead,
    at: Revision,
    id: &str,
    change: Change,
) -> (SketchRead, Option<Offer>) {
    let (after, made) = f.read_at();
    assert_eq!(
        at.next(),
        Some(made),
        "the change is the one between the reads"
    );
    let placement = placement_of(before, at, made, id, change);
    let offered = offer(&after, made, &placement);
    (after, offered)
}

/// The overlapping groups' points (the study's): p1 (10, 0), p2 (30, 0), p3 (50, 0); and a
/// free line's ends, (10, 20) and (30, 25).
const OVERLAPPING_POINTS: &str = r#"{"id": "pa000001", "x": 10, "y": 0, "construction": false}, {"id": "pa000002", "x": 30, "y": 0, "construction": false}, {"id": "pa000003", "x": 50, "y": 0, "construction": false}, {"id": "pa000004", "x": 10, "y": 20, "construction": false}, {"id": "pa000005", "x": 30, "y": 25, "construction": false}"#;

/// p1-p2, p2-p3, and the free line.
const OVERLAPPING_LINES: &str = r#"{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000004", "to": "pa000005", "construction": false}"#;

/// The five rows of two overlapping dependent groups (the study's), each a constraint's JSON
/// less its ID: y1 = 0, y2 = 0, `horizontal` p1-p2, y3 = 0, `horizontal` p2-p3. The groups
/// share y2 = 0.
const OVERLAPPING_ROWS: [&str; 5] = [
    r#""type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true"#,
    r#""type": "distance_y", "a": "origin", "b": "pa000002", "value": 0, "driving": true"#,
    r#""type": "horizontal", "line": "na000001""#,
    r#""type": "distance_y", "a": "origin", "b": "pa000003", "value": 0, "driving": true"#,
    r#""type": "horizontal", "line": "na000002""#,
];

/// The study's IDs for the five rows, in their order.
const STUDY_ORDER: [&str; 5] = ["ka000001", "ka000002", "ka000003", "ka000004", "ka000005"];

/// The overlapping groups, each row `i` with the ID `ids[i]`, then the constraints `more` (a
/// JSON list's insides, or nothing).
fn overlapping_with(ids: [&str; 5], more: &str) -> String {
    let constraints: Vec<String> = OVERLAPPING_ROWS
        .iter()
        .zip(ids)
        .map(|(row, id)| format!(r#"{{"id": "{id}", {row}}}"#))
        .chain((!more.is_empty()).then(|| more.to_owned()))
        .collect();
    sketch_doc(
        OVERLAPPING_POINTS,
        OVERLAPPING_LINES,
        &constraints.join(", "),
    )
}

/// The overlapping groups with the study's IDs, then the constraints `more`.
fn overlapping(more: &str) -> String {
    overlapping_with(STUDY_ORDER, more)
}

/// Every order of `ids`, each once: 120 of five.
fn orders(ids: [&'static str; 5]) -> Vec<[&'static str; 5]> {
    fn from(k: usize, order: &mut [&'static str; 5], all: &mut Vec<[&'static str; 5]>) {
        if k == order.len() {
            all.push(*order);
            return;
        }
        for i in k..order.len() {
            order.swap(k, i);
            from(k + 1, order, all);
            order.swap(k, i);
        }
    }
    let mut all = Vec::new();
    from(0, &mut ids.clone(), &mut all);
    all
}

/// The free line's length as drawn, 5√17 ≈ 20.616: (10, 20) to (30, 25).
fn free_length() -> Form {
    Form::Length {
        line: Reference::Element(element("na000003")),
        value: Length::rounded(f64::sqrt(20.0 * 20.0 + 5.0 * 5.0)).unwrap(),
        driving: true,
    }
}

#[test]
fn an_independent_dimension_on_overlapping_dependent_groups_is_never_offered_in_any_order() {
    // PlaneGCS's contest flags members of the two groups by popularity, then the fewer rows,
    // then the later tag, so which it flags follows the rows' IDs. In each of the 120 ways
    // the IDs ka000001 to ka000005 can be given to the five rows, a driving length of the free
    // line, as drawn, added with its ID first (00000001) or last (zzzzzzzz) in the tags: it
    // takes one freedom and joins no group, so it's never offered, though in some the
    // contest's flags move (counted: some must, or the case isn't real). One session, each
    // order written over the last.
    let f = opened_with(ToolKind::Line, &overlapping(""));
    let mut moved = 0;
    for order in orders(STUDY_ORDER) {
        for id in ["00000001", "zzzzzzzz"] {
            replace(&f, overlapping_with(order, ""));
            let (before, at) = f.read_at();
            assert_eq!(refusal(&before), None, "{order:?}: {:?}", before.states);
            add_constraint(&f, id, free_length());
            let (after, offered) = judged(&f, &before, at, id, Change::Add);
            assert_eq!(after.states.dofs, before.states.dofs - 1, "{order:?} {id}");
            moved += usize::from(flagged_in(&after) != flagged_in(&before));
            assert!(
                offered.is_none(),
                "{order:?} {id}: {offered:?}, {:?}",
                after.states
            );
        }
    }
    assert!(moved > 0, "the contest's flags never moved");
}

#[test]
fn an_independent_dimension_the_contests_pick_moves_for_is_not_offered() {
    // One of the orders above, named: y1 = 0 ka000002, y2 = 0 ka000003, `horizontal` p1-p2
    // ka000001, y3 = 0 ka000004, `horizontal` p2-p3 ka000005. The free line's length, as
    // drawn, added as zzzzzzzz, the last tag: it takes one freedom and joins no group, yet
    // PlaneGCS's contest moves its flag from ka000003 to ka000002. The old rule offered it for
    // that newly flagged ka000002, a dimension that over-defines nothing; `offer`'s rule, c3
    // as amended (Fable, 2026-11-09, c3-refused-sketch-fable.md), reads its freedoms, not the
    // contest's flags: not offered.
    let order = ["ka000002", "ka000003", "ka000001", "ka000004", "ka000005"];
    let f = opened_with(ToolKind::Line, &overlapping_with(order, ""));
    let (before, at) = f.read_at();
    assert_eq!(refusal(&before), None, "{:?}", before.states);
    add_constraint(&f, "zzzzzzzz", free_length());
    let (after, offered) = judged(&f, &before, at, "zzzzzzzz", Change::Add);
    assert_eq!(after.states.dofs, before.states.dofs - 1);
    assert_eq!(
        (flagged_in(&before), flagged_in(&after)),
        (
            BTreeSet::from([element("ka000003")]),
            BTreeSet::from([element("ka000002")])
        ),
        "the contest's pick moved"
    );
    assert!(offered.is_none(), "{offered:?}, {:?}", after.states);
}

#[test]
fn a_dependent_dimension_on_the_same_sketch_is_offered() {
    // y3 = 0 again, by a second driving distance: dependent (the freedoms stay), offered,
    // its ID first or last.
    for id in ["00000001", "zzzzzzzz"] {
        let f = opened_with(ToolKind::Line, &overlapping(""));
        let (before, at) = f.read_at();
        add_constraint(
            &f,
            id,
            Form::DistanceY {
                a: Reference::Origin,
                b: Reference::Element(element("pa000003")),
                value: Length::default(),
                driving: true,
            },
        );
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(after.states.dofs, before.states.dofs, "{id}");
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(id)),
            "{id}: {:?}",
            after.states
        );
    }
}

#[test]
fn a_dimension_joining_a_conflict_is_offered() {
    // The free line's length 30, and 31: a conflict, refused. A third length, 32, joins the
    // conflicting group: its own state is conflicting, and its row is dependent, so the
    // freedoms stay; offered, its ID first or last.
    let more = format!(
        "{}, {}",
        length_json("kb000001", "na000003", "30"),
        length_json("kb000002", "na000003", "31")
    );
    for id in ["00000001", "zzzzzzzz"] {
        let f = opened_with(ToolKind::Line, &overlapping(&more));
        let (before, at) = f.read_at();
        assert_eq!(
            refusal(&before),
            Some(ErrorCode::OverConstrained),
            "{:?}",
            before.states
        );
        add_constraint(&f, id, length_of("na000003", "32"));
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(
            state_of(&after, id),
            ConstraintState::Conflicting,
            "{id}: {:?}",
            after.states
        );
        assert_eq!(after.states.dofs, before.states.dofs, "{id}");
        assert_eq!(offered.map(|o| o.constraint), Some(element(id)), "{id}");
    }
}

/// The level line nb000001, pb000001 (10, 10) to pb000002 (30, 10), its points.
const LEVEL_POINTS: &str = r#"{"id": "pb000001", "x": 10, "y": 10, "construction": false}, {"id": "pb000002", "x": 30, "y": 10, "construction": false}"#;

const LEVEL_LINE: &str =
    r#"{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}"#;

/// The level line held level (`horizontal` kc000001) and 20 across (`distance_x` kc000002,
/// as drawn).
const LEVEL_CONSTRAINTS: &str = r#"{"id": "kc000001", "type": "horizontal", "line": "nb000001"}, {"id": "kc000002", "type": "distance_x", "a": "pb000001", "b": "pb000002", "value": 20, "driving": true}"#;

/// A free line nb000002 elsewhere, pb000003 (10, 40) to pb000004 (34, 58): 30 long as drawn,
/// 24 across and 18 up. Its points.
const FREE_POINTS: &str = r#"{"id": "pb000003", "x": 10, "y": 40, "construction": false}, {"id": "pb000004", "x": 34, "y": 58, "construction": false}"#;

const FREE_LINE: &str =
    r#"{"id": "nb000002", "from": "pb000003", "to": "pb000004", "construction": false}"#;

/// The free line's two lengths, kd000001 `a` and kd000002 `b`.
fn free_lengths(a: &str, b: &str) -> String {
    format!(
        "{}, {}",
        length_json("kd000001", "nb000002", a),
        length_json("kd000002", "nb000002", b)
    )
}

/// The level line and the free line, eight unknowns, the level line's two constraints, then
/// the constraints `more`.
fn level(more: &str) -> String {
    sketch_doc(
        &format!("{LEVEL_POINTS}, {FREE_POINTS}"),
        &format!("{LEVEL_LINE}, {FREE_LINE}"),
        &format!("{LEVEL_CONSTRAINTS}, {more}"),
    )
}

/// A triangle of three free lines, as Ctrl draws them (no relations): pe000001 (10, 10),
/// pe000002 (20, 10), and pe000003 (15, 18.660254), each side 10 to the nanometre.
const TRIANGLE_POINTS: &str = r#"{"id": "pe000001", "x": 10, "y": 10, "construction": false}, {"id": "pe000002", "x": 20, "y": 10, "construction": false}, {"id": "pe000003", "x": 15, "y": 18.660254, "construction": false}"#;

/// Its bottom ne000001, its right side ne000002, its left side ne000003.
const TRIANGLE_LINES: &str = r#"{"id": "ne000001", "from": "pe000001", "to": "pe000002", "construction": false}, {"id": "ne000002", "from": "pe000002", "to": "pe000003", "construction": false}, {"id": "ne000003", "from": "pe000003", "to": "pe000001", "construction": false}"#;

/// The triangle's bottom and right side 10 long (ke000001, ke000002), and its bottom 10 again
/// (ke000003), agreeing: six unknowns, the rank 2, so 4 freedoms, and the pair {ke000001,
/// ke000003} dependent, PlaneGCS's contest leaving out the later tag, ke000003, redundant: a
/// driving dimension repeating another, so over defined, solved.
const TRIANGLE_LENGTHS: &str = r#"{"id": "ke000001", "type": "length", "line": "ne000001", "value": 10, "driving": true}, {"id": "ke000002", "type": "length", "line": "ne000002", "value": 10, "driving": true}, {"id": "ke000003", "type": "length", "line": "ne000001", "value": 10, "driving": true}"#;

#[test]
fn a_repeated_dimension_added_on_a_refused_sketch_is_offered_whatever_its_id() {
    // #429 round 1, R1. Eight unknowns; the rows the horizontal, the x distance, and the
    // free line's lengths 30 and 31, which conflict, so the solve is refused
    // `over_constrained` and the rank is 3: 5 freedoms. A `length` of the level line, 20, repeats the x distance
    // (the line level, its row is the x distance's): the rank stays 3, 5 freedoms, so it's
    // dependent, offered, its ID first or last. Its group is {kc000002, it}, a row each, so the
    // contest leaves out the later tag: its ID first, kc000002 reads redundant and it
    // `unsolved`, which its own state alone wouldn't offer; last, it reads redundant.
    let mut states = Vec::new();
    for id in ["00000001", "zzzzzzzz"] {
        let f = opened_with(ToolKind::Line, &level(&free_lengths("30", "31")));
        let (before, at) = f.read_at();
        assert_eq!(
            (refusal(&before), before.states.dofs),
            (Some(ErrorCode::OverConstrained), 5),
            "{:?}",
            before.states
        );
        add_constraint(&f, id, length_of("nb000001", "20"));
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(
            (refusal(&after), after.states.dofs),
            (Some(ErrorCode::OverConstrained), 5),
            "{id}: {:?}",
            after.states
        );
        assert_eq!(
            listed(&after, ConstraintState::Conflicting),
            ["kd000001", "kd000002"],
            "{id}"
        );
        states.push((state_of(&after, "kc000002"), state_of(&after, id)));
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(id)),
            "{id}: {:?}",
            after.states
        );
    }
    use ConstraintState::{Redundant, Unsolved};
    assert_eq!(states, [(Redundant, Unsolved), (Unsolved, Redundant)]);
}

#[test]
fn a_dimension_a_micrometre_off_its_partner_added_on_an_over_defined_sketch_is_offered_whatever_its_id()
 {
    // #429 round 1, R1's second case, on the level line (something free): decision 5's
    // window. The level sketch with the free line's lengths 30 and 30 again, agreeing: the
    // later tag, kd000002, redundant, a driving dimension repeating another, so over
    // defined, solved; eight unknowns, the rank 3, 5 freedoms. A `length` of the level line, 20.000001, a micrometre off its x distance
    // kc000002, whose row it repeats: within PlaneGCS's redundancy (its error's square under
    // 1e-10), so the rank stays 3 and the freedoms 5, but past decision 5's tolerance (1e-9 of
    // the level line's 20 mm), so the core refuses the pair's pick, `over_constrained`, naming
    // it alone, the later tag: kc000002 with the length's ID first, the length with it last.
    // Dependent: offered either way, though with its ID first it's `unsolved`, which its own
    // state alone wouldn't offer, and the sketch was over defined already.
    let mut named = Vec::new();
    for id in ["00000001", "zzzzzzzz"] {
        let f = opened_with(ToolKind::Line, &level(&free_lengths("30", "30")));
        let (before, at) = f.read_at();
        assert_eq!(
            (before.states.state, before.states.dofs, refusal(&before)),
            (SketchState::OverDefined, 5, None),
            "{:?}",
            before.states
        );
        assert_eq!(listed(&before, ConstraintState::Redundant), ["kd000002"]);
        add_constraint(&f, id, length_of("nb000001", "20.000001"));
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(
            (refusal(&after), after.states.dofs),
            (Some(ErrorCode::OverConstrained), 5),
            "{id}: {:?}",
            after.states
        );
        let problem = after.states.solve_problem.as_ref().unwrap();
        assert_eq!(
            problem.elements(),
            listed(&after, ConstraintState::Conflicting),
            "{id}: {:?}",
            after.states
        );
        named.extend(problem.elements().iter().cloned());
        assert_eq!(listed(&after, ConstraintState::Redundant), ["kd000002"]);
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(id)),
            "{id}: {:?}",
            after.states
        );
    }
    assert_eq!(named, ["kc000002", "zzzzzzzz"]);
}

#[test]
fn a_dimension_a_micrometre_off_others_added_where_nothing_is_free_is_offered_whatever_its_id() {
    // #429 round 1, R1's second case, on the rectangle (nothing free). The rectangle fully
    // defined, 0 freedoms; a third dimension, the top left to the bottom right across, 50 as
    // drawn: over defined, solved, 0 freedoms still (nothing free, so PlaneGCS counts the rows it didn't
    // find redundant, and its group was). A fourth across, 50.000001, a micrometre off the
    // third, whose row it repeats. Its group (the top's length, the right side's `vertical`,
    // the third, and it) has two rows too many, and which two the contest leaves out follows
    // the tags:
    // - its ID first, PlaneGCS resolves the group as redundant, so the freedoms stay 0, and
    //   decision 5 (1e-9 of the rectangle's 53.9 mm diagonal) refuses the fourth alone, the
    //   pick that disagrees: dependent and flagged itself;
    // - its ID last, the two left out leave the third and the fourth both in the reduced
    //   solve, which fails, so PlaneGCS leaves the group in conflict, all four listed, and
    //   counts every row: -2 freedoms; flagged itself.
    // Offered either way.
    for id in ["00000000", "zzzzzzzz"] {
        let (mut f, mut tool) = dimensioned();
        third(&mut f, &mut tool, "50");
        let (before, at) = f.read_at();
        assert_eq!(
            (before.states.state, before.states.dofs, refusal(&before)),
            (SketchState::OverDefined, 0, None),
            "{:?}",
            before.states
        );
        add_constraint(
            &f,
            id,
            Form::DistanceX {
                a: Reference::Element(top_left(&before)),
                b: Reference::Element(bottom_right(&before)),
                value: Length::parse("50.000001").unwrap(),
                driving: true,
            },
        );
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        let conflicting = listed(&after, ConstraintState::Conflicting);
        let first = id == "00000000";
        assert_eq!(
            (refusal(&after), after.states.dofs, conflicting.len()),
            (
                Some(ErrorCode::OverConstrained),
                if first { 0 } else { -2 },
                if first { 1 } else { 4 }
            ),
            "{id}: {:?}",
            after.states
        );
        assert!(conflicting.contains(&id), "{id}: {conflicting:?}");
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(id)),
            "{id}: {:?}",
            after.states
        );
    }
}

/// The level sketch with the free line's lengths 30 and 30.000001, disagreeing past decision
/// 5's tolerance (1e-9 of the free line's 30 mm), so refused `over_constrained`, the later
/// tag, kd000002, conflicting; and the level line's length `e`, 20, agreeing with its x
/// distance kc000002, one of the two redundant, the later tag.
fn agreeing_pair(e: &str) -> String {
    level(&format!(
        "{}, {}",
        free_lengths("30", "30.000001"),
        length_json(e, "nb000001", "20")
    ))
}

/// Of the agreeing pair, the one PlaneGCS's contest leaves out with `e` as the length: the
/// later tag.
fn pick(e: &str) -> &str {
    if e > "kc000002" { e } else { "kc000002" }
}

/// [`agreeing_pair`] opened and read: refused by the free line, the pair's pick redundant.
fn opened_pair(e: &str) -> (Fixture, SketchRead, Revision) {
    let f = opened_with(ToolKind::Line, &agreeing_pair(e));
    let (before, at) = f.read_at();
    assert_eq!(
        refusal(&before),
        Some(ErrorCode::OverConstrained),
        "{:?}",
        before.states
    );
    assert_eq!(listed(&before, ConstraintState::Conflicting), ["kd000002"]);
    assert_eq!(listed(&before, ConstraintState::Redundant), [pick(e)]);
    (f, before, at)
}

#[test]
fn a_redundant_dimension_edited_into_conflict_on_a_refused_sketch_is_offered_whatever_its_id() {
    // #429 round 1, R2. The sketch refused by the free line; the level line's length, E, and
    // its x distance, P, agreeing, the later tag redundant. E edited to 21, a millimetre off: an
    // edit changes a value, not a row, so the group and the contest's pick stay, and PlaneGCS's
    // reduced solve, the pick left out, can't meet it, so the pair's group stays in conflict,
    // both listed: newly in conflict, offered, E's ID first or last. Where E is the later tag
    // it was redundant before, so its own state newly flagged wouldn't offer it.
    for e in ["zzzzzzzz", "00000001"] {
        let (f, before, at) = opened_pair(e);
        set_value(&f, e, "21");
        let (after, offered) = judged(&f, &before, at, e, Change::Edit);
        assert_eq!(
            (state_of(&after, e), state_of(&after, "kc000002")),
            (ConstraintState::Conflicting, ConstraintState::Conflicting),
            "{e}: {:?}",
            after.states
        );
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(e)),
            "{e}: {:?}",
            after.states
        );
    }
}

#[test]
fn a_dimension_edited_a_micrometre_off_its_partner_on_a_refused_sketch_is_offered_whatever_its_id()
{
    // #429 round 1, R2, in decision 5's window. The same sketch; E edited to 20.000001, a
    // micrometre off P: within PlaneGCS's redundancy, past decision 5's tolerance (1e-9 of the
    // level line's 20 mm, 2e-8 mm), so the core refuses the pair's pick alone,
    // newly conflicting: E where its ID is the later tag, P where it's the earlier, so the
    // runs differ in which. Offered either way: by E's own state alone neither would be (E,
    // the pick, was flagged before; E, not the pick, isn't flagged after). Edited back to 20,
    // agreeing again: nothing newly in conflict, the solve refused before and after, not
    // offered.
    let mut newly = Vec::new();
    for e in ["00000001", "zzzzzzzz"] {
        let (f, before, at) = opened_pair(e);
        set_value(&f, e, "20.000001");
        let (after, offered) = judged(&f, &before, at, e, Change::Edit);
        assert_eq!(
            refusal(&after),
            Some(ErrorCode::OverConstrained),
            "{e}: {:?}",
            after.states
        );
        let conflicting = listed(&after, ConstraintState::Conflicting);
        let new: Vec<&str> = conflicting
            .iter()
            .copied()
            .filter(|k| !listed(&before, ConstraintState::Conflicting).contains(k))
            .collect();
        assert_eq!(new, [pick(e)], "{e}: {:?}", after.states);
        newly.push(pick(e).to_owned());
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(e)),
            "{e}: {:?}",
            after.states
        );
        let at = at.next().unwrap();
        set_value(&f, e, "20");
        let (back, offered) = judged(&f, &after, at, e, Change::Edit);
        assert_eq!(listed(&back, ConstraintState::Conflicting), ["kd000002"]);
        assert_eq!(offered, None, "{e}: {:?}", back.states);
    }
    assert_eq!(newly, ["kc000002", "zzzzzzzz"]);
}

#[test]
fn an_edit_that_ends_its_conflict_is_offered_where_decision_5_then_finds_another_whatever_its_id() {
    // The edit rule's known case (`offer_of`'s doc; c3-edit-masking-fable.md, §1.1 and §4). A
    // constraint's conflict has two judges: PlaneGCS, and decision 5, which runs only where
    // PlaneGCS lists no conflict. The same sketch; E edited to 21, a millimetre off P, its
    // row's partner (the line level, its length is its x distance): past PlaneGCS's
    // redundancy (1 mm squared against 1e-10), so PlaneGCS lists the pair whole, E and
    // kc000002, and decision 5 doesn't run, so kd000002, 30.000001 against 30 (1e-6 mm, its
    // square within PlaneGCS's redundancy), reads `redundant`, its disagreement masked:
    // newly in conflict, offered. Then E edited back to 20: the pair agrees, PlaneGCS lists
    // no conflict, and decision 5 runs, judging kd000002's micrometre past its part's
    // tolerance (1e-9 of the free line's 30 mm, 3e-8 mm): conflicting, newly, so offered, E's
    // ID first or last. That's the uncovering, not the edit's doing: kd000002 disagreed
    // before it, unjudged. E is dependent, agreeing with its group again, so the offer is
    // true of it, and wrong only in laying the conflict to the edit. The same edit back to 20
    // from 20.000001 (the test above) isn't offered: decision 5 judged kd000002 on that read
    // too. Eight unknowns, the rank 3, so 5 freedoms throughout: an edit changes no row, and
    // a refused answer keeps the drawing's diagnosis.
    for e in ["00000001", "zzzzzzzz"] {
        let (f, before, at) = opened_pair(e);
        set_value(&f, e, "21");
        let (after, offered) = judged(&f, &before, at, e, Change::Edit);
        assert_eq!(
            (refusal(&after), after.states.dofs),
            (Some(ErrorCode::OverConstrained), 5),
            "{e}: {:?}",
            after.states
        );
        let mut pair = vec![e, "kc000002"];
        pair.sort_unstable();
        assert_eq!(listed(&after, ConstraintState::Conflicting), pair, "{e}");
        assert_eq!(
            listed(&after, ConstraintState::Redundant),
            ["kd000002"],
            "{e}"
        );
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(e)),
            "{e}: {:?}",
            after.states
        );
        let at = at.next().unwrap();
        set_value(&f, e, "20");
        let (back, offered) = judged(&f, &after, at, e, Change::Edit);
        assert_eq!(
            (refusal(&back), back.states.dofs),
            (Some(ErrorCode::OverConstrained), 5),
            "{e}: {:?}",
            back.states
        );
        assert_eq!(
            listed(&back, ConstraintState::Conflicting),
            ["kd000002"],
            "{e}"
        );
        assert_eq!(listed(&back, ConstraintState::Redundant), [pick(e)], "{e}");
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(e)),
            "{e}: {:?}",
            back.states
        );
    }
}

#[test]
fn an_add_that_newly_refuses_an_over_defined_sketch_is_offered_whatever_its_id() {
    // #429 round 1, R3. The triangle, over defined and solved, 4 freedoms. Its left side
    // given 30: no triangle has sides 10, 10, and 30, so the reduced solve (the pick left
    // out) fails, and PlaneGCS leaves the pair's group in conflict, ke000001 and ke000003
    // both listed; the new length is in no group, `unsolved`, and the rank 3, so 3 freedoms;
    // refused `over_constrained`. Neither its freedoms (lower) nor its own state offer it; the
    // solve newly refused does, its ID first or last.
    for id in ["00000001", "zzzzzzzz"] {
        let f = opened_with(
            ToolKind::Line,
            &sketch_doc(TRIANGLE_POINTS, TRIANGLE_LINES, TRIANGLE_LENGTHS),
        );
        let (before, at) = f.read_at();
        assert_eq!(
            (before.states.state, before.states.dofs, refusal(&before)),
            (SketchState::OverDefined, 4, None),
            "{:?}",
            before.states
        );
        assert_eq!(listed(&before, ConstraintState::Redundant), ["ke000003"]);
        add_constraint(&f, id, length_of("ne000003", "30"));
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(
            (refusal(&after), after.states.dofs),
            (Some(ErrorCode::OverConstrained), 3),
            "{id}: {:?}",
            after.states
        );
        assert_eq!(
            listed(&after, ConstraintState::Conflicting),
            ["ke000001", "ke000003"],
            "{id}"
        );
        assert_eq!(state_of(&after, id), ConstraintState::Unsolved, "{id}");
        assert_eq!(
            offered.map(|o| o.constraint),
            Some(element(id)),
            "{id}: {:?}",
            after.states
        );
    }
}

#[test]
fn an_independent_add_that_fails_a_refused_sketchs_reduced_solve_is_never_offered_a_known_miss() {
    // c3 as amended's residual risk, its §4's first item, `offer`'s known miss, pinned as
    // determinism. The triangle and the free line with lengths 30 and 31, in conflict:
    // refused `over_constrained`, ten unknowns, the rank 3, so 7 freedoms; the free line's
    // pair conflicting, the triangle's pick redundant. The left side given 30, independent:
    // the reduced solve fails, so every group is listed conflicting, the new length in none
    // (`unsolved`), the rank 4, 6 freedoms. The solve was refused already, its freedoms
    // dropped, and it isn't flagged: never offered, its ID first or last, and the sketch's
    // state says what's wrong.
    for id in ["00000001", "zzzzzzzz"] {
        let doc = sketch_doc(
            &format!("{TRIANGLE_POINTS}, {FREE_POINTS}"),
            &format!("{TRIANGLE_LINES}, {FREE_LINE}"),
            &format!("{TRIANGLE_LENGTHS}, {}", free_lengths("30", "31")),
        );
        let f = opened_with(ToolKind::Line, &doc);
        let (before, at) = f.read_at();
        assert_eq!(
            (refusal(&before), before.states.dofs),
            (Some(ErrorCode::OverConstrained), 7),
            "{:?}",
            before.states
        );
        assert_eq!(
            listed(&before, ConstraintState::Conflicting),
            ["kd000001", "kd000002"]
        );
        assert_eq!(listed(&before, ConstraintState::Redundant), ["ke000003"]);
        add_constraint(&f, id, length_of("ne000003", "30"));
        let (after, offered) = judged(&f, &before, at, id, Change::Add);
        assert_eq!(
            (refusal(&after), after.states.dofs),
            (Some(ErrorCode::OverConstrained), 6),
            "{id}: {:?}",
            after.states
        );
        assert_eq!(
            listed(&after, ConstraintState::Conflicting),
            ["kd000001", "kd000002", "ke000001", "ke000003"],
            "{id}"
        );
        assert_eq!(state_of(&after, id), ConstraintState::Unsolved, "{id}");
        assert_eq!(offered, None, "{id}: {:?}", after.states);
    }
}

#[test]
fn a_value_edit_that_takes_a_solved_sketch_to_no_solution_is_offered_once() {
    // The free line's length 30, and its ends' x distance 24 (as drawn): solved, under
    // defined (eight unknowns, four rows). The length edited to 15, shorter than the 24
    // across: no solution, refused `solve_failed`, newly: offered. Edited again, to 14: no
    // solution still, nothing in conflict, so the second edit isn't offered; nor is the first
    // on the second's read, a later revision (the moment's past).
    let more = format!(
        r#"{}, {{"id": "kd000002", "type": "distance_x", "a": "pb000003", "b": "pb000004", "value": 24, "driving": true}}"#,
        length_json("kd000001", "nb000002", "30")
    );
    let f = opened_with(ToolKind::Line, &level(&more));
    let (before, at) = f.read_at();
    assert_eq!(
        (before.states.state, before.states.dofs, refusal(&before)),
        (SketchState::UnderDefined, 4, None),
        "{:?}",
        before.states
    );
    set_value(&f, "kd000001", "15");
    let (first, offered) = judged(&f, &before, at, "kd000001", Change::Edit);
    assert_eq!(
        (first.states.state, refusal(&first)),
        (SketchState::NoSolution, Some(ErrorCode::SolveFailed)),
        "{:?}",
        first.states
    );
    assert!(offered.is_some(), "{:?}", first.states);
    let made = at.next().unwrap();
    set_value(&f, "kd000001", "14");
    let (second, offered) = judged(&f, &first, made, "kd000001", Change::Edit);
    assert_eq!(
        (second.states.state, refusal(&second)),
        (SketchState::NoSolution, Some(ErrorCode::SolveFailed)),
        "{:?}",
        second.states
    );
    assert_eq!(offered, None, "{:?}", second.states);
    let later = made.next().unwrap();
    let first_edit = placement_of(&before, at, made, "kd000001", Change::Edit);
    assert_eq!(offer(&second, later, &first_edit), None);
}

#[test]
fn a_dimension_that_takes_a_freedom_from_a_solved_sketch_is_never_offered() {
    // Over chains the Line tool draws (each from the origin or not, along the axes, level,
    // plumb, and slanted, and with segments in line, whose relations make overlapping
    // dependent groups once the distances fix their points), each driving distance x and y
    // from the origin to each point, as drawn, added one at a time, their IDs before every
    // other's in the tags and after: where the sketch solved and the add took exactly one
    // freedom, SD7 offers nothing, whatever the contest flags. Among those, the contest's
    // flags move for some (counted: some must, or the sweep proves nothing of them).
    let chains: [&[(f64, f64)]; 9] = [
        &[
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 10.0),
            (10.0, 10.0),
            (10.0, 30.0),
            (0.0, 30.0),
            (0.0, 0.0),
        ],
        &[(5.0, 5.0), (25.0, 5.2), (25.3, 30.0), (45.0, 41.0)],
        &[
            (0.0, 0.0),
            (20.0, 0.1),
            (35.0, 12.0),
            (35.2, 40.0),
            (0.1, 40.0),
        ],
        &[(-20.0, 10.0), (0.1, 10.0), (0.0, 30.0), (-15.0, 45.0)],
        &[
            (10.0, -10.0),
            (30.0, -10.2),
            (30.0, 0.0),
            (50.0, 0.1),
            (50.2, 20.0),
        ],
        &[
            (12.0, 7.0),
            (31.0, 18.0),
            (47.0, 9.0),
            (47.0, 30.0),
            (12.0, 30.3),
            (12.0, 7.0),
        ],
        &[(5.0, 5.0), (5.0, 25.0), (5.0, 45.0), (25.0, 45.0)],
        &[
            (0.0, 10.0),
            (20.0, 10.0),
            (40.0, 10.0),
            (40.0, 30.0),
            (0.0, 30.0),
            (0.0, 10.0),
        ],
        &[
            (10.0, 10.0),
            (30.0, 10.0),
            (50.0, 10.0),
            (70.0, 10.0),
            (70.0, 30.0),
        ],
    ];
    let mut judged_count = 0;
    let mut moved = 0;
    for (chain, last) in chains.iter().flat_map(|c| [(c, false), (c, true)]) {
        let mut f = opened(ToolKind::Line);
        for (x, y) in *chain {
            f.click(*x, *y);
        }
        f.send(Event::Enter);
        let mut n = 0u32;
        let points: Vec<(ElementId, f64, f64)> = f
            .read()
            .members
            .points
            .iter()
            .map(|p| (p.id.clone(), p.x.mm(), p.y.mm()))
            .collect();
        for (p, x, y) in points {
            for (along_x, value) in [(true, x), (false, y)] {
                let (before, at) = f.read_at();
                n += 1;
                let id = if last {
                    format!("zzz{n:05}")
                } else {
                    format!("{:08}", 70_000_000 + n)
                };
                let (a, b) = (Reference::Origin, Reference::Element(p.clone()));
                let value = Length::rounded(value).unwrap();
                let form = if along_x {
                    Form::DistanceX {
                        a,
                        b,
                        value,
                        driving: true,
                    }
                } else {
                    Form::DistanceY {
                        a,
                        b,
                        value,
                        driving: true,
                    }
                };
                add_constraint(&f, &id, form);
                let (after, offered) = judged(&f, &before, at, &id, Change::Add);
                if after.states.solve_problem.is_none()
                    && before.states.solve_problem.is_none()
                    && after.states.dofs == before.states.dofs - 1
                {
                    judged_count += 1;
                    moved += usize::from(flagged_in(&after) != flagged_in(&before));
                    assert!(offered.is_none(), "{id}: {offered:?}, {:?}", after.states);
                }
            }
        }
    }
    assert!(judged_count >= 20, "{judged_count} judged");
    assert!(
        moved > 0,
        "of {judged_count} judged, the contest's flags moved for none"
    );
}

// --- The constraint tools (`relate`) on the real solve ------------------------------------

/// A line ended on a circle by an `on`, as a line drawn to a circle's rim is left: the line
/// ne000001 from pe000001 (0, 15) to pe000002 (20, 15), its end held by kt000001 on the
/// circle ce000001 about pe000003 (20, 10), radius 5, whose top (20, 15) it is.
fn ended_on() -> String {
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{"id": "pe000001", "x": 0, "y": 15, "construction": false}, {"id": "pe000002", "x": 20, "y": 15, "construction": false}, {"id": "pe000003", "x": 20, "y": 10, "construction": false}], "lines": [{"id": "ne000001", "from": "pe000001", "to": "pe000002", "construction": false}], "circles": [{"id": "ce000001", "center": "pe000003", "radius": 5, "construction": false}], "arcs": [], "constraints": [{"id": "kt000001", "type": "on", "point": "pe000002", "curve": "ce000001"}]}]}"#.to_owned()
}

/// `members` sent as a `sketch_remove`, which must be taken.
fn sketch_remove(f: &Fixture, members: Vec<Reference>) {
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let removed = f
        .sessions
        .sketch_remove(&s1(), revision, sketch_id(), members)
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("sketch_remove didn't answer in {HANG:?}"));
    assert!(removed.result.unwrap_or_else(|p| panic!("{p:?}")).changed);
}

#[test]
fn tangent_over_an_on_replaces_it_and_the_solve_flags_nothing_where_the_add_alone_flags_one() {
    // C5's reason, on PlaneGCS. Unknowns: the line's two points, four; the circle's centre
    // and radius, three: seven. The `on` is one equation: six freedoms, nothing flagged.
    // Tangent of [ne000001, ce000001]: the tool's add of the tangent_on (its end on the
    // circle, and the angle there: two equations, FreeCAD's endpoint-to-edge tangency), then
    // the removal of kt000001. After both: five freedoms, nothing flagged.
    let mut f = opened_with(ToolKind::Line, &ended_on());
    let read = f.read();
    assert_eq!(read.states.dofs, 6);
    assert!(
        flagged_in(&read).is_empty(),
        "{:?}",
        read.states.constraints
    );
    let picks = [
        Reference::Element(element("ne000001")),
        Reference::Element(element("ce000001")),
    ];
    let related = relate(&read.members, RelationTool::Tangent, &picks, &mut f.ids).unwrap();
    assert_eq!(related.already, Vec::<ElementId>::new());
    let Some(RelateEdit::AddThenRemove { add, remove }) = related.edit else {
        panic!("{related:?}");
    };
    assert_eq!(remove, vec![element("kt000001")]);
    let [tangent] = add.constraints.as_slice() else {
        panic!("{add:?}");
    };
    assert_eq!(
        tangent.form,
        Form::TangentOn {
            a: Reference::Element(element("ne000001")),
            at: Reference::Element(element("pe000002")),
            b: Reference::Element(element("ce000001")),
            direction: silico_core::document::Direction::Same,
        }
    );
    f.sketch_add(&add);
    sketch_remove(&f, remove.into_iter().map(Reference::Element).collect());
    let read = f.read();
    assert_eq!(read.states.dofs, 5);
    assert_eq!(read.members.constraints.len(), 1);
    assert!(
        flagged_in(&read).is_empty(),
        "{:?}",
        read.states.constraints
    );
    assert_eq!(
        state_of(&read, tangent.id.as_str()),
        ConstraintState::Satisfied
    );

    // The same add without the removal: the `on` repeats the tangent_on's end on the circle,
    // so the freedoms are the same five, and one of the two is flagged, whichever PlaneGCS
    // picks.
    let f = opened_with(ToolKind::Line, &ended_on());
    f.sketch_add(&add);
    let read = f.read();
    assert_eq!(read.states.dofs, 5);
    assert_eq!(read.members.constraints.len(), 2);
    assert_eq!(flagged_in(&read).len(), 1, "{:?}", read.states.constraints);
}
