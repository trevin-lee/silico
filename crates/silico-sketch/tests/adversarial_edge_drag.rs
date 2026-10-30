//! break-it, M5.3 open item 17: an edge's drag (docs/ports/sketcher.md, *Dragging an
//! edge*), in this process (`silico_sketch::Drag`, the worker's `State`) and through the
//! core's `drag::sketch::begin_edge` on the real sketch worker. Expected values are FreeCAD
//! 1.1.3's (`moveGeometry(geo, 0, V, relative)`, each case's script quoted where it's
//! asserted), worked by hand (a free line moves by the cursor's offset from the press; a
//! free circle's radius is the cursor's distance from its centre), or the drag's own
//! promises (an end's sketch passes the gate and its load gives it back bit for bit).

use super::systems;

use std::path::Path;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{DragStop, EdgeGrab, FrameKind, Opened, SketchEnded, begin_edge};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, SketchWorker, solve};
use silico_protocol::{
    Algorithm, DragBegin, Geometry, MAX_DRAG_UNKNOWNS, MovedOnce, Op, PointOf, PointValues, Reply,
    SketchGeometry, SketchSystem, SolveStatus, SolverConstraint,
};
use silico_sketch::Drag;
use silico_sketch::worker::State;
use systems::Builder;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver stops when its step is under 1e-10.
const CLOSE: f64 = 1e-9;

fn at(values: &[f64], p: PointValues) -> [f64; 2] {
    [values[p.x.0 as usize], values[p.y.0 as usize]]
}

fn near(got: [f64; 2], want: [f64; 2], tol: f64, what: &str) {
    let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
    assert!(
        off <= tol,
        "{what}: got {got:?}, want {want:?} ({off:e} off)"
    );
}

fn edge_drag(system: SketchSystem, g: Geometry) -> Drag {
    let start = Drag::begin(&DragBegin::edge(system, g)).unwrap();
    assert!(start.conflicting.is_empty(), "{:?}", start.conflicting);
    start.drag.unwrap()
}

/// A circle about `c`, radius `r`: its centre fixed values if `fixed_centre`, its radius
/// held by a Radius constraint if `held`.
fn circle_at(c: (f64, f64), r: f64, fixed_centre: bool, held: bool) -> (SketchSystem, Geometry) {
    let mut b = Builder::default();
    let center = if fixed_centre {
        PointValues {
            x: b.fixed(c.0),
            y: b.fixed(c.1),
        }
    } else {
        b.point_values(c.0, c.1)
    };
    let radius = b.unknown(r);
    let circle = b.add(SketchGeometry::Circle { center, radius });
    if held {
        let v = b.fixed(r);
        b.constrain(|tag| SolverConstraint::Radius {
            tag,
            circle,
            value: v,
        });
    }
    (b.system(Algorithm::DogLeg), circle)
}

/// The quarter arc of drag.rs: about (1,2), radius 5, from (6,2) to (1,7), nothing held.
fn quarter_arc() -> (
    SketchSystem,
    Geometry,
    PointValues,
    PointValues,
    PointValues,
) {
    let mut b = Builder::default();
    let center = b.point_values(1.0, 2.0);
    let radius = b.unknown(5.0);
    let start_angle = b.unknown(0.0);
    let end_angle = b.unknown(std::f64::consts::FRAC_PI_2);
    let start = b.point_values(6.0, 2.0);
    let end = b.point_values(1.0, 7.0);
    let arc = b.add(SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constraints.push(SolverConstraint::ArcRules { arc });
    (b.system(Algorithm::DogLeg), arc, center, start, end)
}

// --- In this process ------------------------------------------------------------------

#[test]
fn arc_edge_to_1_10_leaves_its_ends_where_freecad_does() {
    // 1.1.3: ArcOfCircle(Circle((1,2), z, 5), 0, π/2); moveGeometry(0, 0, (1,10), False):
    // r = 8, centre (1,2), start (8.972216512, 1.333843948), end (1.078316940, 9.999616644).
    let (system, arc, center, start, end) = quarter_arc();
    let mut drag = edge_drag(system, arc);
    let moved = drag.move_to([1.0, 10.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    near(at(&v, center), [1.0, 2.0], 1e-6, "centre");
    near(at(&v, start), [8.972216512, 1.333843948], 1e-6, "start");
    near(at(&v, end), [1.078316940, 9.999616644], 1e-6, "end");
}

#[test]
fn arc_edge_dragged_to_its_far_side_leaves_its_ends_where_freecad_does() {
    // 1.1.3: the same arc, moveGeometry(0, 0, (-7, 2), False): r = 8, centre (1,2),
    // start (8.998232055, 2.168178481), end (1.625909758, 9.975477226).
    let (system, arc, center, start, end) = quarter_arc();
    let mut drag = edge_drag(system, arc);
    let moved = drag.move_to([-7.0, 2.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    near(at(&v, center), [1.0, 2.0], 1e-6, "centre");
    assert!((v[2] - 8.0).abs() <= 1e-6, "radius {}", v[2]);
    near(at(&v, start), [8.998232055, 2.168178481], 1e-6, "start");
    near(at(&v, end), [1.625909758, 9.975477226], 1e-6, "end");
}

#[test]
fn circle_edge_dragged_onto_its_own_centre_is_no_success_and_the_drag_goes_on() {
    // 1.1.3 refuses the move ("Not able to move point with the id and type: (0, 0)"): the
    // pin's gradient at the centre is 0/0. A frame there mustn't be a Success, and mustn't
    // end the drag.
    let (system, circle) = circle_at((0.0, 0.0), 5.0, false, false);
    let mut drag = edge_drag(system, circle);
    match drag.move_to([0.0, 0.0]) {
        Ok(moved) => assert_ne!(moved.status, SolveStatus::Success, "{moved:?}"),
        Err(failure) => {
            // A structured refusal is fine; a bridge throw would end a worker's drag.
            assert!(
                !matches!(failure.refusal, silico_protocol::Refusal::SolverThrew),
                "{failure:?}"
            );
        }
    }
    let moved = drag.move_to([0.0, 8.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    assert!((moved.values.unwrap()[2] - 8.0).abs() <= CLOSE);
}

#[test]
fn circle_edge_with_fixed_centre_sizes_it_through_the_cursor() {
    // 1.1.3: the centre held at the origin (DistanceX, DistanceY), moveGeometry(0, 0, (6,8)):
    // radius 10.
    let (system, circle) = circle_at((0.0, 0.0), 5.0, true, false);
    let mut drag = edge_drag(system, circle);
    let moved = drag.move_to([6.0, 8.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    assert!((moved.values.unwrap()[2] - 10.0).abs() <= CLOSE);
}

#[test]
fn fully_held_circles_edge_moves_nothing() {
    // 1.1.3: centre held at the origin, radius held at 5, moveGeometry(0, 0, (0,8)): no
    // error, the circle as it was.
    let (system, circle) = circle_at((0.0, 0.0), 5.0, true, true);
    let mut drag = edge_drag(system, circle);
    let moved = drag.move_to([0.0, 8.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    assert_eq!(&v[..3], &[0.0, 0.0, 5.0]);
}

#[test]
fn circle_near_a_kilometre_out_whose_top_is_past_reach_drags() {
    // About (0, 9.99e5), radius 5e3: its top, where the pin's target starts, is 1.004e6,
    // past reach. The first move puts the target at the cursor, so it's never solved there.
    let (system, circle) = circle_at((0.0, 9.99e5), 5e3, false, false);
    let mut drag = edge_drag(system, circle);
    let moved = drag.move_to([0.0, 9.99e5 - 6e3]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    assert!((v[2] - 6e3).abs() <= 1e-6, "radius {}", v[2]);
    near([v[0], v[1]], [0.0, 9.99e5], 1e-6, "centre");
}

#[test]
fn line_spanning_reach_dragged_past_it_is_refused_and_the_drag_goes_on() {
    let mut b = Builder::default();
    let line = b.line((-1e6, 0.0), (1e6, 0.0));
    let mut drag = edge_drag(b.system(Algorithm::DogLeg), line);
    // The midpoint (0,0) to (1,0): the end to 1e6 + 1.
    assert!(drag.move_to([1.0, 0.0]).is_err());
    let moved = drag.move_to([0.0, 3.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    assert_eq!(v, [-1e6, 3.0, 1e6, 3.0]);
}

#[test]
fn zero_length_line_by_value_dragged_is_no_success() {
    // Two distinct points at one place: dragged whole, it's still no line, so the frame
    // can't be a Success (or the begin is refused).
    let mut b = Builder::default();
    let line = b.line((5.0, 5.0), (5.0, 5.0));
    let begin = DragBegin::edge(b.system(Algorithm::DogLeg), line);
    let Ok(start) = Drag::begin(&begin) else {
        return;
    };
    let mut drag = start.drag.unwrap();
    let moved = drag.move_to([7.0, 9.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::SolvedInvalid, "{moved:?}");
}

#[test]
fn line_edge_whose_end_many_lines_share_takes_them_and_leaves_their_far_ends() {
    // A line (0,0)-(10,0) whose end is 60 free lines' start (the end values shared), each
    // to (10 + cos, sin): dragged by (3, 4), the hub goes to (13, 4), every far end stays.
    let mut b = Builder::default();
    let start = b.point_values(0.0, 0.0);
    let hub = b.point_values(10.0, 0.0);
    let line = b.add(SketchGeometry::Line { start, end: hub });
    let mut far = Vec::new();
    for i in 0..60 {
        let a = f64::from(i) * 0.1;
        let p = b.point_values(10.0 + libm::cos(a), libm::sin(a));
        b.add(SketchGeometry::Line { start: hub, end: p });
        far.push((p, [10.0 + libm::cos(a), libm::sin(a)]));
    }
    let mut drag = edge_drag(b.system(Algorithm::DogLeg), line);
    let moved = drag.move_to([8.0, 4.0]).unwrap();
    assert_eq!(moved.status, SolveStatus::Success);
    let v = moved.values.unwrap();
    near(at(&v, start), [3.0, 4.0], CLOSE, "start");
    near(at(&v, hub), [13.0, 4.0], CLOSE, "hub");
    for (p, want) in far {
        near(at(&v, p), want, CLOSE, "a far end");
    }
}

#[test]
fn line_edge_in_a_part_past_the_drag_limit_is_refused_but_moves_once() {
    // 65 lines of length 1 chained end to start by coincidences: 260 unknowns in one part,
    // past 256.
    let mut b = Builder::default();
    let mut lines = Vec::new();
    let one = b.fixed(1.0);
    for i in 0..65 {
        let x = f64::from(i);
        let line = b.line((x, 0.0), (x + 1.0, 0.0));
        lines.push(line);
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(line),
            b: PointOf::End(line),
            value: one,
        });
    }
    for w in lines.windows(2) {
        let (a, c) = (w[0], w[1]);
        b.constrain(|tag| SolverConstraint::Coincident {
            tag,
            a: PointOf::End(a),
            b: PointOf::Start(c),
        });
    }
    let begin = DragBegin::edge(b.system(Algorithm::DogLeg), lines[0]);
    assert!(begin.part().unknowns.len() > MAX_DRAG_UNKNOWNS);
    let Err(failure) = Drag::begin(&begin) else {
        panic!("a drag past the limit began")
    };
    let text = format!("{failure:?}");
    assert!(text.contains("256") || text.contains("unknowns"), "{text}");
    // Named as the request named it: the curve, not a point it never sent (the reviews).
    let refused = DragBegin::edge(begin.system.clone(), lines[0])
        .check()
        .unwrap_err();
    assert_eq!(refused.parameter, "curve");
    // As one move it's held to a solve's limits, not a drag's.
    let mut state = State::default();
    match state
        .handle(Op::MoveOnce {
            begin,
            to: [0.5, 2.0],
        })
        .unwrap()
    {
        Reply::MovedOnce(MovedOnce::Moved(moved)) => {
            assert_eq!(moved.status, SolveStatus::Success);
            let v = moved.values.unwrap();
            near([v[1], v[2]], [0.0, 2.0], CLOSE, "start");
            near([v[3], v[4]], [1.0, 2.0], CLOSE, "end");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn move_once_of_a_lines_edge_puts_its_midpoint_at_the_cursor() {
    // 1.1.3's absolute form: moveGeometry(0, none, (5,4)) of (0,0)-(10,0) gives (0,4)-(10,4).
    let mut b = Builder::default();
    let line = b.line((0.0, 0.0), (10.0, 0.0));
    let mut state = State::default();
    match state
        .handle(Op::MoveOnce {
            begin: DragBegin::edge(b.system(Algorithm::DogLeg), line),
            to: [5.0, 4.0],
        })
        .unwrap()
    {
        Reply::MovedOnce(MovedOnce::Moved(moved)) => {
            assert_eq!(moved.values.unwrap(), [0.0, 4.0, 10.0, 4.0]);
        }
        other => panic!("{other:?}"),
    }
    // And a point's edge, or geometry past the end, is refused, the worker still serving.
    let mut b = Builder::default();
    let p = b.fixed_point(1.0, 1.0);
    let system = b.system(Algorithm::DogLeg);
    for g in [p, Geometry(7), Geometry(u32::MAX)] {
        assert!(
            state
                .handle(Op::MoveOnce {
                    begin: DragBegin::edge(system.clone(), g),
                    to: [0.0, 0.0],
                })
                .is_err(),
            "{g:?}"
        );
    }
    let mut b = Builder::default();
    let line = b.line((0.0, 0.0), (10.0, 0.0));
    assert!(matches!(
        state.handle(Op::MoveOnce {
            begin: DragBegin::edge(b.system(Algorithm::DogLeg), line),
            to: [5.0, 1.0],
        }),
        Ok(Reply::MovedOnce(MovedOnce::Moved(_)))
    ));
}

#[test]
fn an_edge_drags_frames_are_the_same_bits_twice() {
    let (arc_system, arc, ..) = quarter_arc();
    for (system, g) in [(arc_system, arc), circle_at((3.0, -2.0), 5.0, false, true)] {
        let run = || {
            let mut drag = edge_drag(system.clone(), g);
            [[1.0, 10.0], [-3.3, 4.7], [0.1, 0.2]].map(|c| {
                drag.move_to(c)
                    .unwrap()
                    .values
                    .map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>())
            })
        };
        assert_eq!(run(), run());
    }
}

fn biased() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        -1e3..1e3f64,
        -1.0..1.0f64,
        -1e-6..1e-6f64,
        (-9e5..9e5f64),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// A free line's edge, its midpoint moved to the cursor: each end moves by the cursor's
    /// offset from the midpoint, exactly as its pins say (the line is the only geometry).
    #[test]
    fn a_free_lines_edge_moves_by_the_cursors_offset(
        ax in biased(), ay in biased(), bx in biased(), by in biased(),
        cx in biased(), cy in biased(),
    ) {
        prop_assume!(libm::hypot(bx - ax, by - ay) > 1e-6);
        let mut b = Builder::default();
        let line = b.line((ax, ay), (bx, by));
        let mut drag = edge_drag(b.system(Algorithm::DogLeg), line);
        let mid = [(ax + bx) * 0.5, (ay + by) * 0.5];
        let d = [cx - mid[0], cy - mid[1]];
        let want_a = [ax + d[0], ay + d[1]];
        let want_b = [bx + d[0], by + d[1]];
        let reach = |p: [f64; 2]| p.iter().all(|v| v.abs() <= 1e6);
        match drag.move_to([cx, cy]) {
            Ok(moved) => {
                prop_assert_eq!(moved.status, SolveStatus::Success);
                let v = moved.values.unwrap();
                let scale = 1e-9 * (1.0 + [ax, ay, bx, by, cx, cy].iter().fold(0.0f64, |m, x| m.max(x.abs())));
                for (got, want) in [([v[0], v[1]], want_a), ([v[2], v[3]], want_b)] {
                    let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
                    prop_assert!(off <= scale, "got {:?} want {:?}", got, want);
                }
            }
            Err(f) => prop_assert!(!(reach(want_a) && reach(want_b)), "refused in reach: {:?}", f),
        }
    }

    /// A free circle's edge: its radius the cursor's distance from its centre, the centre
    /// staying, for a cursor not at the centre.
    #[test]
    fn a_free_circles_edge_passes_through_the_cursor(
        x in biased(), y in biased(), r in prop_oneof![Just(1e-6), 1e-3..1e3f64, Just(5e5)],
        cx in biased(), cy in biased(),
    ) {
        let (system, circle) = circle_at((x, y), r, false, false);
        let mut drag = edge_drag(system, circle);
        let d = libm::hypot(cx - x, cy - y);
        prop_assume!(d > 1e-6 * (1.0 + x.abs().max(y.abs())));
        match drag.move_to([cx, cy]) {
            Ok(moved) if moved.status == SolveStatus::Success => {
                let v = moved.values.unwrap();
                let tol = 1e-9 * (1.0 + x.abs().max(y.abs()).max(cx.abs()).max(cy.abs()));
                // The centre moves by at most 1e-4 of the change (its pin's weight squared).
                prop_assert!((libm::hypot(cx - v[0], cy - v[1]) - v[2]).abs() <= tol,
                    "the cursor off the circle: {:?}", v);
                prop_assert!(libm::hypot(v[0] - x, v[1] - y) <= 1e-3 * (d - r).abs() + tol,
                    "the centre moved: {:?}", v);
            }
            // Within 2^19 mm every residual can reach DogLeg's 1e-10, and every move
            // succeeds; past it one ulp of a length is more than that, and a move succeeds or
            // not by its last bit of rounding, in 1.1.3 too (the sketcher note's *Dragging an
            // edge*, the port review's oracle rates). Kept under 5e5 for margin.
            Ok(moved) => {
                let far = [x, y, cx, cy, d, r].iter().any(|v| v.abs() >= 5e5);
                prop_assert!(far, "a failed frame within 2^19 mm: {:?}", moved);
                prop_assert!(moved.values.is_none(), "{:?}", moved);
            }
            // Refused only for a radius past reach.
            Err(f) => prop_assert!(d > 1e6 * (1.0 - 1e-9), "refused: {:?}", f),
        }
    }
}

// --- Through the core, on the worker --------------------------------------------------

fn document(feature: &str) -> Checked {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{feature}]}}"#
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn solvers() -> SketchSolvers {
    SketchSolvers::starting(|| {
        Ok(Box::new(SketchWorker::start_worker(Path::new(WORKER))?) as Box<dyn SketchSolver>)
    })
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn only_feature(doc: &Checked) -> FeatureId {
    doc.document().features[0].id.clone()
}

fn sketch_of(doc: &Checked) -> Sketch {
    match &doc.document().features[0].operation {
        Operation::Sketch(sketch) => sketch.clone(),
        _ => panic!("not a sketch"),
    }
}

fn point_of(sketch: &Sketch, p: &str) -> [f64; 2] {
    let p = sketch.points.iter().find(|q| q.id.as_str() == p).unwrap();
    [p.x.mm(), p.y.mm()]
}

fn edge(
    solvers: &SketchSolvers,
    doc: &Checked,
    curve: &str,
    press: [f64; 2],
) -> Result<Opened, silico_core::Error> {
    begin_edge(
        solvers,
        None,
        doc,
        &only_feature(doc),
        EdgeGrab {
            curve: &id(curve),
            press,
        },
        None,
        &DragStop::new(),
    )
}

/// An end keeps its promises: kept the cursor's, the sketch passes the gate and its load
/// gives it back bit for bit; kept the begin's, the file as it was, saying why.
fn check_end(doc: &Checked, ended: &SketchEnded, what: &str) {
    let file = sketch_of(doc);
    match ended.kept {
        Kept::Cursor => {
            assert_eq!(ended.why, None, "{what}");
            let mut edited = doc.document().clone();
            edited.features[0].operation = Operation::Sketch(ended.sketch.clone());
            let edited = Checked::new(edited)
                .unwrap_or_else(|e| panic!("{what}: the gate refuses the end's sketch: {e:?}"));
            let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
            let again = solve(&edited, &only_feature(doc), &mut worker)
                .unwrap_or_else(|e| panic!("{what}: the end's sketch doesn't solve: {e:?}"));
            assert_eq!(again.written(&ended.sketch), ended.sketch, "{what}: drift");
        }
        Kept::Begin => {
            assert_eq!(ended.sketch, file, "{what}");
            assert!(!ended.changed, "{what}");
        }
    }
}

/// A free line pe000001 (0,0)-pe000002 (10,0), and a free circle about pe000003 (20,0),
/// radius 5, as document_drag.rs's FREE; the line a construction line if `construction`.
fn free(construction: bool) -> Checked {
    document(&format!(
        r#"{{"id": "s0000005", "type": "sketch", "v": 1, "name": "Free", "plane": "xy", "points": [{{"id": "pe000001", "x": 0, "y": 0, "construction": false}}, {{"id": "pe000002", "x": 10, "y": 0, "construction": false}}, {{"id": "pe000003", "x": 20, "y": 0, "construction": false}}], "lines": [{{"id": "ne000001", "from": "pe000001", "to": "pe000002", "construction": {construction}}}], "circles": [{{"id": "ce000001", "center": "pe000003", "radius": 5, "construction": {construction}}}], "arcs": [], "constraints": []}}"#
    ))
}

/// A line pz000001 (-5e5, 0)-pz000002 (5e5, 0), each end's x held (distance_x from it to
/// the origin, and from the origin to it): it can move only up and down.
const LONG: &str = r#"{"id": "s000000z", "type": "sketch", "v": 1, "name": "Long", "plane": "xy", "points": [{"id": "pz000001", "x": -500000, "y": 0, "construction": false}, {"id": "pz000002", "x": 500000, "y": 0, "construction": false}], "lines": [{"id": "nz000001", "from": "pz000001", "to": "pz000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "kz000001", "type": "distance_x", "a": "pz000001", "b": "origin", "value": 500000, "driving": true}, {"id": "kz000002", "type": "distance_x", "a": "origin", "b": "pz000002", "value": 500000, "driving": true}]}"#;

#[test]
fn edge_press_that_isnt_a_number_is_refused_at_begin_naming_it() {
    // Found by break-it, fixed (the press is checked at begin, naming it): begin_edge took a NaN press: the drag opens, and every move is then refused
    // as a cursor "got NaN" naming `to`, a value the caller never sent; the end too.
    let solvers = solvers();
    let doc = free(false);
    for press in [
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
        [f64::NEG_INFINITY, 1.0],
    ] {
        match edge(&solvers, &doc, "ne000001", press) {
            Ok(Opened { drag, .. }) => {
                let frame = drag.move_to([5.0, 4.0]).map(|t| t.wait());
                panic!("began with press {press:?}; a move to (5, 4): {frame:?}")
            }
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
                assert_eq!(e.parameter(), Some("press"), "{e:?}");
            }
        }
    }
}

#[test]
fn edge_press_out_of_reach_is_refused_at_begin_naming_it() {
    // Found by break-it, fixed (as above): a press 2 km out was taken; the drag's every move is refused naming `to`, the
    // cursor quoted as the shifted value (2e6 off what was sent).
    let solvers = solvers();
    let doc = free(false);
    match edge(&solvers, &doc, "ne000001", [2e6, 0.0]) {
        Ok(Opened { drag, .. }) => {
            let moved = drag.move_to([5.0, 4.0]).map(|t| t.wait());
            panic!("began with a press out of reach; a move to (5, 4): {moved:?}")
        }
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
            assert_eq!(e.parameter(), Some("press"), "{e:?}");
        }
    }
}

#[test]
fn long_lines_edge_moved_up_with_cursor_and_result_in_reach_isnt_refused() {
    // Found by break-it, fixed (the press goes to the worker, which follows the cursor itself, so only the caller's cursor is checked for reach): pressed near its right end (4.9e5, 0), the cursor taken to (-6e5, 5): both in
    // reach, and so is the answer, the line up 5 (its ends' x held): (-5e5, 5)-(5e5, 5).
    // 1.1.3 (scaled by 1/5): DistanceX-held (-1e5,0)-(1e5,0), moveGeometry(0, 0, (-1.1e5, 5),
    // True) gives (-1e5, 5)-(1e5, 5). Silico shifts the cursor by the midpoint's offset from
    // the press, (-1.09e6, 5), past reach, and refuses the frame and the end.
    let solvers = solvers();
    let doc = document(LONG);
    let Opened { drag, .. } = edge(&solvers, &doc, "nz000001", [4.9e5, 0.0]).unwrap();
    let frame = drag
        .move_to([-6e5, 5.0])
        .and_then(|t| t.wait())
        .unwrap_or_else(|e| panic!("the move was refused: {e:?}"));
    assert_eq!(frame.kind, FrameKind::Solved, "{frame:?}");
    let ended = drag
        .end(Some([-6e5, 5.0]))
        .unwrap_or_else(|e| panic!("the end was refused: {e:?}"));
    assert_eq!(ended.kept, Kept::Cursor, "{:?}", ended.why);
    let s = &ended.sketch;
    assert!((point_of(s, "pz000001")[1] - 5.0).abs() <= 1e-6);
    assert!((point_of(s, "pz000002")[1] - 5.0).abs() <= 1e-6);
    check_end(&doc, &ended, "long line");
}

#[test]
fn circle_edge_released_at_or_a_hair_from_its_centre_keeps_a_sketch_the_gate_takes() {
    // 1.1.3: to the centre exactly is refused; to 1e-9 above it leaves a radius of 1e-9.
    // Silico's end must keep the begin saying why, or a sketch the gate and a load take.
    let solvers = solvers();
    let doc = free(false);
    for dy in [0.0, 1e-12, 1e-9, 1e-7, 1.000_000_1e-7, 1e-6] {
        let Opened { drag, .. } = edge(&solvers, &doc, "ce000001", [20.0, 5.0]).unwrap();
        let frame = drag.move_to([20.0, dy]).unwrap().wait().unwrap();
        if frame.kind == FrameKind::Solved {
            let r = frame.radii[0].1;
            assert!(r > 0.0, "dy {dy}: radius {r}");
        }
        let ended = drag
            .end(Some([20.0, dy]))
            .unwrap_or_else(|e| panic!("dy {dy}: {e:?}"));
        if ended.kept == Kept::Begin {
            assert!(
                ended.why.is_some(),
                "dy {dy}: kept the begin saying nothing"
            );
        }
        check_end(&doc, &ended, &format!("dy {dy}"));
    }
}

#[test]
fn construction_curves_edges_drag_as_any_curves_do() {
    let solvers = solvers();
    let doc = free(true);
    let Opened { drag, .. } = edge(&solvers, &doc, "ne000001", [2.0, 0.0]).unwrap();
    let ended = drag.end(Some([5.0, 4.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert_eq!(point_of(&ended.sketch, "pe000001"), [3.0, 4.0]);
    assert_eq!(point_of(&ended.sketch, "pe000002"), [13.0, 4.0]);
    assert!(ended.sketch.lines[0].construction);
    check_end(&doc, &ended, "construction line");
    let Opened { drag, .. } = edge(&solvers, &doc, "ce000001", [20.0, 5.0]).unwrap();
    let ended = drag.end(Some([20.0, 8.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor);
    assert!((ended.sketch.circles[0].radius.mm() - 8.0).abs() <= CLOSE);
    check_end(&doc, &ended, "construction circle");
}

/// The bracket profile of document_drag.rs, with its leg's thickness held too: every point
/// held.
const HELD: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

#[test]
fn fully_held_sketchs_line_edge_drags_and_ends_unchanged() {
    let solvers = solvers();
    let doc = document(HELD);
    let Opened { drag, dofs, .. } = edge(&solvers, &doc, "na000003", [30.0, 5.0]).unwrap();
    assert_eq!(dofs, 0);
    let frame = drag.move_to([30.0, 20.0]).unwrap().wait().unwrap();
    assert_ne!(frame.kind, FrameKind::Invalid);
    let ended = drag.end(Some([30.0, 20.0])).unwrap();
    assert!(!ended.changed, "{ended:?}");
    check_end(&doc, &ended, "held");
}

/// A free arc about pr000001 (0,0), from pr000002 (5,0) to pr000003 (0,5), and a radius-5
/// arc held, sharing an end with a line: the arcs' edges, dragged and ended.
const FREE_ARC: &str = r#"{"id": "s000000a", "type": "sketch", "v": 1, "name": "Free arc", "plane": "xy", "points": [{"id": "pr000001", "x": 0, "y": 0, "construction": false}, {"id": "pr000002", "x": 5, "y": 0, "construction": false}, {"id": "pr000003", "x": 0, "y": 5, "construction": false}, {"id": "pr000004", "x": -10, "y": 5, "construction": false}], "lines": [{"id": "nr000001", "from": "pr000003", "to": "pr000004", "construction": false}], "circles": [], "arcs": [{"id": "ar000001", "center": "pr000001", "start": "pr000002", "end": "pr000003", "construction": false}], "constraints": [{"id": "kr000001", "type": "horizontal", "line": "nr000001"}]}"#;

#[test]
fn an_arcs_edge_dragged_anywhere_ends_on_a_sketch_its_load_gives_back() {
    let solvers = solvers();
    let doc = document(FREE_ARC);
    for to in [
        [3.0, 9.0],
        [-7.0, -7.0],
        [0.0, 0.0],
        [1e-9, 0.0],
        [9e5, 9e5],
        [0.0, 1e6],
        [-3.0, 4.0],
    ] {
        let Opened { drag, .. } = edge(&solvers, &doc, "ar000001", [3.0, 4.0]).unwrap();
        let ended = drag
            .end(Some(to))
            .unwrap_or_else(|e| panic!("{to:?}: {e:?}"));
        if ended.kept == Kept::Begin && to != [3.0, 4.0] {
            assert!(ended.why.is_some(), "{to:?}: kept the begin saying nothing");
        }
        check_end(&doc, &ended, &format!("{to:?}"));
    }
}

#[test]
fn an_edges_failed_end_leaves_the_pool_able_to_drag_again() {
    let solvers = solvers();
    let doc = free(false);
    let Opened { drag, .. } = edge(&solvers, &doc, "ne000001", [2.0, 0.0]).unwrap();
    // Out of reach: refused, the begin kept.
    assert!(drag.end(Some([2e6, 0.0])).is_err());
    let Opened { drag, .. } = edge(&solvers, &doc, "ne000001", [2.0, 0.0]).unwrap();
    let ended = drag.end(Some([5.0, 4.0])).unwrap();
    assert_eq!(point_of(&ended.sketch, "pe000002"), [13.0, 4.0]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A free line pressed anywhere along it, released anywhere: it ends moved by the
    /// release's offset from the press, its load giving it back; pressed off it, refused.
    #[test]
    fn a_lines_edge_ends_moved_by_the_releases_offset_from_the_press(
        t in prop_oneof![Just(0.0), Just(1.0), Just(0.5), 0.0..1.0f64],
        off in prop_oneof![Just(0.0), -1e3..1e3f64],
        cx in biased(), cy in biased(),
    ) {
        let solvers = solvers();
        let doc = free(false);
        let press = [10.0 * t, off];
        let begun = edge(&solvers, &doc, "ne000001", press);
        // Off the line by more than the length tolerance, the press is refused, naming it
        // (#307: a pick lands on its edge); `t` keeps it within the line's ends, so `off` is
        // its distance.
        if off.abs() > silico_protocol::LENGTH_TOLERANCE {
            let refused = begun.err().unwrap();
            prop_assert_eq!(refused.code(), ErrorCode::InvalidInput);
            prop_assert_eq!(refused.parameter(), Some("press"));
            return Ok(());
        }
        let Opened { drag, .. } = begun.unwrap();
        let ended = drag.end(Some([cx, cy])).unwrap();
        check_end(&doc, &ended, "line");
        if [cx, cy] == press {
            prop_assert_eq!(ended.kept, Kept::Begin);
        } else {
            prop_assert_eq!(ended.kept, Kept::Cursor);
            let d = [cx - press[0], cy - press[1]];
            let tol = 1e-6 * (1.0 + cx.abs().max(cy.abs()));
            let a = point_of(&ended.sketch, "pe000001");
            let b = point_of(&ended.sketch, "pe000002");
            prop_assert!(libm::hypot(a[0] - d[0], a[1] - d[1]) <= tol, "{:?}", a);
            prop_assert!(libm::hypot(b[0] - 10.0 - d[0], b[1] - d[1]) <= tol, "{:?}", b);
        }
    }
}

#[test]
fn a_line_whose_end_an_arc_ends_at_moved_up_takes_the_arc_as_freecad_does() {
    // 1.1.3: ArcOfCircle(Circle((0,0), z, 5), 0, π/2); a line (0,5)-(-10,5), Coincident
    // (0,2)-(1,1), Horizontal(1); moveGeometry(1, 0, (0,2), True): the line (0,7)-(-10,7),
    // the arc about (-0.388189584, 1.152720427), radius 5.860150985, from
    // (5.373193561, 0.081350360). Silico shares the end (M3.2's D2) and pins the line's ends
    // alone, as FreeCAD does; the arc, held by nothing, lands where the solve leaves it.
    let solvers = solvers();
    let doc = document(FREE_ARC);
    let Opened { drag, .. } = edge(&solvers, &doc, "nr000001", [-5.0, 5.0]).unwrap();
    let ended = drag.end(Some([-5.0, 7.0])).unwrap();
    assert_eq!(ended.kept, Kept::Cursor, "{:?}", ended.why);
    let s = &ended.sketch;
    let at = |p| point_of(s, p);
    near(at("pr000003"), [0.0, 7.0], 1e-6, "the shared end");
    near(at("pr000004"), [-10.0, 7.0], 1e-6, "the line's far end");
    near(
        at("pr000001"),
        [-0.388189584, 1.152720427],
        1e-9,
        "the arc's centre",
    );
    near(
        at("pr000002"),
        [5.373193561, 0.081350360],
        1e-9,
        "the arc's start",
    );
}
