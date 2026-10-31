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
use silico_core::document::{ElementId, FeatureId};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use silico_core::sketch_states::{ConstraintState, SketchState};
use silico_core::sketch_tools::{Context, Event, Modifiers, NewIds, SketchTool, ToolKind};
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

/// Sessions on a fake kernel, each sketch solved by the sketch worker, with the empty
/// sketch open as s1; and a tool drawing on it.
struct Fixture {
    _dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    tool: SketchTool,
    ids: Counter,
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
            let revision = self.sessions.status(&s1()).session.unwrap().revision;
            let added = self
                .sessions
                .sketch_add(
                    &s1(),
                    revision,
                    sketch_id(),
                    NewMembers::of(&members).unwrap(),
                )
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("sketch_add didn't answer in {HANG:?}"));
            let made = added.session.as_ref().unwrap().revision;
            let added = added.result.unwrap_or_else(|p| panic!("{p:?}"));
            assert!(added.changed);
            self.tool.added(token, made);
        }
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
