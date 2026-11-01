//! M5.11's sketch tools (silico_core::sketch_tools) on the real core: each gesture driven
//! against the session's newest `sketch` read, its `sketch_add` sent through the sessions'
//! own command, and the sketch the rebuild then solves on PlaneGCS, in this crate's worker,
//! read back. So the core takes what the tools make, and the sketch solves as worked by hand:
//! each figure's degrees of freedom counted from its unknowns (two a point, an arc's own rules
//! in PlaneGCS's terms: a free arc has five) less its equations, its solved points where the
//! tool drew them (the tools draw what their relations hold), and its profile's regions.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use silico_core::cache::Cache;
use silico_core::command::sketch::SketchRead;
use silico_core::command::{Call, ModelChoice, NewMembers, Revision, SessionId, Sessions};
use silico_core::document::sketch_edit::Members;
use silico_core::document::{ElementId, FeatureId, Form, Length, Reference};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use silico_core::sketch_states::{ConstraintState, SketchState};
use silico_core::sketch_tools::{
    Context, DimensionEvent, Event, Modifiers, NewIds, Offer, OfferChoice, OfferRequest,
    SketchTool, SmartDimension, ToolKind, offer,
};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// The pick tolerance, in millimetres.
const TOLERANCE: f64 = 0.5;

/// An empty sketch on the xy plane, the document's only feature.
const EMPTY: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;

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

fn opened(kind: ToolKind) -> Fixture {
    opened_with(kind, EMPTY)
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
fn a_redundant_horizontal_the_pointer_showed_is_kept_and_read_redundant() {
    // #278's G: a line with both ends on the x axis, and horizontal. Four unknowns, two
    // independent equations (each end's y): two freedoms, and one of the three relations
    // repeats the others, read as redundant; the sketch's state by its freedoms, under
    // defined (M5.0's item 32, Addendum 2).
    let mut f = opened(ToolKind::Line);
    f.drag([10.0, 0.1], [40.0, -0.1]);
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
        };
        let ids: &mut dyn NewIds = if self.sort_first {
            &mut self.first
        } else {
            &mut self.ids
        };
        let outcome = tool.handle(&context, &read.states, ids, event.clone());
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
        };
        tool.settle(&context, &read.states);
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
