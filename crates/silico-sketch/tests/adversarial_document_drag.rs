//! break-it, M5.3: `silico_core::drag::sketch`, a document's sketch point dragged by its ID,
//! on the real sketch worker. Expected values are the drag's own promises checked
//! independently (an end's sketch passes the gate and its own load gives it back bit for
//! bit, so the next rebuild finds no drift; the edit is a function of the begin state and
//! the final cursor alone, whatever moves came before), or worked out by hand (a free
//! line's end at the cursor, a sketch released where it began unchanged).

use std::path::Path;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId, FeatureId, Operation, Sketch, read};
use silico_core::drag::Kept;
use silico_core::drag::sketch::{FrameKind, Opened, SketchEnded, begin};
use silico_core::recompute::SketchSolvers;
use silico_core::sketch::{SketchSolver, SketchWorker, solve};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// The solver stops when its step is under 1e-10.
const CLOSE: f64 = 1e-9;

/// The bracket's L profile with one freedom (the base leg's thickness), as
/// document_drag.rs has it.
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

/// An arc about pc000001 (1, 2), counter-clockwise from pc000002 (6, 2) to pc000003 (1, 7),
/// its radius held at 5.
const ARC: &str = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Arc", "plane": "xy", "points": [{"id": "pc000001", "x": 1, "y": 2, "construction": false}, {"id": "pc000002", "x": 6, "y": 2, "construction": false}, {"id": "pc000003", "x": 1, "y": 7, "construction": false}], "lines": [], "circles": [], "arcs": [{"id": "ac000001", "center": "pc000001", "start": "pc000002", "end": "pc000003", "construction": false}], "constraints": [{"id": "kc000001", "type": "radius", "curve": "ac000001", "value": 5, "driving": true}]}"#;

/// Two arcs of radius 5 held, sharing pd000003 (0, 0): the start of one about pd000001
/// (-5, 0), ending at pd000004 (-5, 5), and the end of one about pd000002 (5, 0), starting
/// at pd000005 (5, 5).
const TWO_ARCS: &str = r#"{"id": "s0000004", "type": "sketch", "v": 1, "name": "Two arcs", "plane": "xy", "points": [{"id": "pd000001", "x": -5, "y": 0, "construction": false}, {"id": "pd000002", "x": 5, "y": 0, "construction": false}, {"id": "pd000003", "x": 0, "y": 0, "construction": false}, {"id": "pd000004", "x": -5, "y": 5, "construction": false}, {"id": "pd000005", "x": 5, "y": 5, "construction": false}], "lines": [], "circles": [], "arcs": [{"id": "ad000001", "center": "pd000001", "start": "pd000003", "end": "pd000004", "construction": false}, {"id": "ad000002", "center": "pd000002", "start": "pd000005", "end": "pd000003", "construction": false}], "constraints": [{"id": "kd000001", "type": "radius", "curve": "ad000001", "value": 5, "driving": true}, {"id": "kd000002", "type": "radius", "curve": "ad000002", "value": 5, "driving": true}]}"#;

/// A circle about pe000001 (0, 0), radius 3 and free, and pe000002 (3, 0) held on it.
/// `CIRCLE_HELD` holds its centre at the origin too.
const CIRCLE: &str = r#"{"id": "s0000005", "type": "sketch", "v": 1, "name": "Circle", "plane": "xy", "points": [{"id": "pe000001", "x": 0, "y": 0, "construction": false}, {"id": "pe000002", "x": 3, "y": 0, "construction": false}], "lines": [], "circles": [{"id": "ce000001", "center": "pe000001", "radius": 3, "construction": false}], "arcs": [], "constraints": [{"id": "ke000001", "type": "on", "point": "pe000002", "curve": "ce000001"}]}"#;

/// A free line from pf000001 (0, 0) to pf000002 (10, 0), no constraints.
const LINE: &str = r#"{"id": "s0000006", "type": "sketch", "v": 1, "name": "Line", "plane": "xy", "points": [{"id": "pf000001", "x": 0, "y": 0, "construction": false}, {"id": "pf000002", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "nf000001", "from": "pf000001", "to": "pf000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;

/// A line given two lengths that can't both hold: 5 and 6.
const CONFLICT: &str = r#"{"id": "s0000007", "type": "sketch", "v": 1, "name": "Conflict", "plane": "xy", "points": [{"id": "pg000001", "x": 0, "y": 0, "construction": false}, {"id": "pg000002", "x": 5, "y": 0, "construction": false}], "lines": [{"id": "ng000001", "from": "pg000001", "to": "pg000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "kg000001", "type": "length", "line": "ng000001", "value": 5, "driving": true}, {"id": "kg000002", "type": "length", "line": "ng000001", "value": 6, "driving": true}]}"#;

const CIRCLE_HELD: &str = r#"{"id": "s0000008", "type": "sketch", "v": 1, "name": "Circle held", "plane": "xy", "points": [{"id": "ph000001", "x": 0, "y": 0, "construction": false}, {"id": "ph000002", "x": 3, "y": 0, "construction": false}], "lines": [], "circles": [{"id": "ch000001", "center": "ph000001", "radius": 3, "construction": false}], "arcs": [], "constraints": [{"id": "kh000001", "type": "on", "point": "ph000002", "curve": "ch000001"}, {"id": "kh000002", "type": "distance_x", "a": "origin", "b": "ph000001", "value": 0, "driving": true}, {"id": "kh000003", "type": "distance_y", "a": "origin", "b": "ph000001", "value": 0, "driving": true}]}"#;

/// pq000001 (-5, 0) and pq000002 (5, 0) symmetric about pq000003, held at the origin.
const SYMMETRIC: &str = r#"{"id": "s0000009", "type": "sketch", "v": 1, "name": "Symmetric", "plane": "xy", "points": [{"id": "pq000001", "x": -5, "y": 0, "construction": false}, {"id": "pq000002", "x": 5, "y": 0, "construction": false}, {"id": "pq000003", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": [{"id": "kq000001", "type": "symmetric", "a": "pq000001", "b": "pq000002", "about": "pq000003"}, {"id": "kq000002", "type": "distance_x", "a": "origin", "b": "pq000003", "value": 0, "driving": true}, {"id": "kq000003", "type": "distance_y", "a": "origin", "b": "pq000003", "value": 0, "driving": true}]}"#;

/// A free arc about pr000001 (0, 0), counter-clockwise from pr000002 (5, 0) to pr000003
/// (0, 5), no constraints.
const FREE_ARC: &str = r#"{"id": "s000000a", "type": "sketch", "v": 1, "name": "Free arc", "plane": "xy", "points": [{"id": "pr000001", "x": 0, "y": 0, "construction": false}, {"id": "pr000002", "x": 5, "y": 0, "construction": false}, {"id": "pr000003", "x": 0, "y": 5, "construction": false}], "lines": [], "circles": [], "arcs": [{"id": "ar000001", "center": "pr000001", "start": "pr000002", "end": "pr000003", "construction": false}], "constraints": []}"#;

/// Two free points, ps000001 (0, 0) and ps000002 (10, 0), a reference distance between them.
const DISTANCE_REF: &str = r#"{"id": "s000000b", "type": "sketch", "v": 1, "name": "Reference", "plane": "xy", "points": [{"id": "ps000001", "x": 0, "y": 0, "construction": false}, {"id": "ps000002", "x": 10, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": [{"id": "ks000001", "type": "distance", "a": "ps000001", "b": "ps000002", "value": 10, "driving": false}]}"#;

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

fn assert_near(got: [f64; 2], want: [f64; 2], what: &str) {
    let off = libm::hypot(got[0] - want[0], got[1] - want[1]);
    assert!(
        off <= CLOSE,
        "{what}: got {got:?}, want {want:?} ({off:e} off)"
    );
}

/// A drag of `point` in `doc`'s one sketch, begun, then ended at `to`, with `moves` sent
/// (and waited for) first.
fn dragged(
    solvers: &SketchSolvers,
    doc: &Checked,
    point: &str,
    moves: &[[f64; 2]],
    to: [f64; 2],
) -> SketchEnded {
    let Opened { drag, .. } = begin(solvers, None, doc, &only_feature(doc), &id(point), None)
        .unwrap_or_else(|e| panic!("{point}: {e:?}"));
    for &m in moves {
        drag.move_to(m).unwrap().wait().unwrap();
    }
    drag.end(Some(to))
        .unwrap_or_else(|e| panic!("{point} to {to:?}: {e:?}"))
}

/// The promises an end keeps, whatever the cursor: kept the cursor's, the sketch passes the
/// gate and its load gives it back bit for bit (no drift at the next rebuild), and
/// `changed` says whether it differs from the file; kept the begin's, the file as it was,
/// unchanged, and why, naming the sketch.
fn check_end(doc: &Checked, ended: &SketchEnded, click: bool, what: &str) {
    let file = sketch_of(doc);
    let feature = only_feature(doc);
    match ended.kept {
        Kept::Cursor => {
            assert!(!click, "{what}: a click kept the cursor");
            assert_eq!(ended.why, None, "{what}");
            assert_eq!(ended.changed, ended.sketch != file, "{what}");
            let mut edited = doc.document().clone();
            edited.features[0].operation = Operation::Sketch(ended.sketch.clone());
            let edited = Checked::new(edited)
                .unwrap_or_else(|e| panic!("{what}: the gate refuses the end's sketch: {e:?}"));
            let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
            let again = solve(&edited, &feature, &mut worker)
                .unwrap_or_else(|e| panic!("{what}: the end's sketch doesn't solve: {e:?}"));
            assert_eq!(
                again.written(&ended.sketch),
                ended.sketch,
                "{what}: the next rebuild finds drift"
            );
        }
        Kept::Begin => {
            assert_eq!(ended.sketch, file, "{what}");
            assert!(!ended.changed, "{what}");
            // A click, released where the point began, moves nothing and needs no why.
            if click {
                assert_eq!(ended.why, None, "{what}");
                return;
            }
            let why = ended
                .why
                .as_ref()
                .unwrap_or_else(|| panic!("{what}: the begin kept, with a cursor, and no why"));
            assert_eq!(why.feature(), Some(feature.as_str()), "{what}: {why:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Targeted.

#[test]
fn a_free_lines_end_dragged_lands_at_the_cursor_and_its_other_end_stays_bit_for_bit() {
    let doc = document(LINE);
    let solvers = solvers();
    let ended = dragged(&solvers, &doc, "pf000002", &[], [-3.25, 7.5]);
    assert_eq!((ended.kept, ended.changed), (Kept::Cursor, true));
    assert_near(
        point_of(&ended.sketch, "pf000002"),
        [-3.25, 7.5],
        "dragged end",
    );
    // pf000001 is in no part with pf000002 (a line joins nothing): it's held, exactly.
    let start = point_of(&ended.sketch, "pf000001");
    assert_eq!(start.map(f64::to_bits), [0.0f64, 0.0].map(f64::to_bits));
    check_end(&doc, &ended, false, "free line");
}

#[test]
fn a_drag_released_where_it_began_leaves_the_sketch_unchanged() {
    // Found by break-it, fixed: the two arcs sharing pd000003 (a file that is its own
    // load, checked below), any of their points dragged and released exactly where it
    // began: the worker's move (SQP from the begin state, every pin already met) answered
    // the centres and pd000003 a few 1e-17 off (arc ad000002 ends at angle π, whose sine
    // isn't 0 in floating point), and the end wrote them, `changed`, a click dirtying a
    // git-native file. A release where the point began is a click: the begin state kept,
    // with no why, and the file as it was.
    let mut moved = Vec::new();
    for (sketch, point, at) in [
        (PROFILE, "pa000003", [60.0, 5.0]),
        (ARC, "pc000003", [1.0, 7.0]),
        (ARC, "pc000001", [1.0, 2.0]),
        (TWO_ARCS, "pd000001", [-5.0, 0.0]),
        (TWO_ARCS, "pd000003", [0.0, 0.0]),
        (TWO_ARCS, "pd000004", [-5.0, 5.0]),
        (CIRCLE, "pe000002", [3.0, 0.0]),
        (LINE, "pf000002", [10.0, 0.0]),
    ] {
        let doc = document(sketch);
        let solvers = solvers();
        // Each file is its own load: solved, it's written back as it is.
        let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
        let solved = solve(&doc, &only_feature(&doc), &mut worker).unwrap();
        assert_eq!(solved.written(&sketch_of(&doc)), sketch_of(&doc));
        let ended = dragged(&solvers, &doc, point, &[], at);
        assert_eq!((ended.kept, &ended.why), (Kept::Begin, &None), "{point}");
        if ended.sketch != sketch_of(&doc) || ended.changed {
            moved.push((point, ended.changed, ended.sketch.points));
        }
    }
    assert!(
        moved.is_empty(),
        "released where they began, these moved: {moved:#?}"
    );
}

#[test]
fn a_sketch_whose_constraints_conflict_is_refused_naming_both() {
    let doc = document(CONFLICT);
    let solvers = solvers();
    let Err(e) = begin(
        &solvers,
        None,
        &doc,
        &only_feature(&doc),
        &id("pg000002"),
        None,
    ) else {
        panic!("a conflicting sketch began a drag")
    };
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e:?}");
    assert_eq!(e.elements(), ["kg000001", "kg000002"], "{e:?}");
    assert_eq!(e.feature(), Some("s0000007"));
    // The solver is given back whole: the next drag, of another document, runs.
    let line = document(LINE);
    let ended = dragged(&solvers, &line, "pf000002", &[], [1.0, 1.0]);
    assert_eq!(ended.kept, Kept::Cursor);
}

#[test]
fn a_line_end_dragged_onto_its_other_end_keeps_the_begin_state_saying_why() {
    // Found by break-it, fixed: the frame is `Invalid` (a line of no length, M3.7's
    // SolvedInvalid), and the end's why was a generic `solve_failed` that didn't say the
    // geometry couldn't be built. It's `invalid_result` now, saying what kinds of geometry
    // can't be; it can't name the line, since the worker withholds an invalid frame's
    // values (M3.7), so the end never has them.
    let doc = document(LINE);
    let solvers = solvers();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &only_feature(&doc),
        &id("pf000002"),
        None,
    )
    .unwrap();
    let frame = drag.move_to([0.0, 0.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Invalid);
    let ended = drag.end(Some([0.0, 0.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    assert_eq!(ended.sketch, sketch_of(&doc));
    let why = ended.why.unwrap();
    assert_eq!(why.feature(), Some("s0000006"));
    assert_eq!(why.code(), ErrorCode::InvalidResult, "{why:?}");
    assert!(why.message().contains("a line of no length"), "{why:?}");
}

#[test]
fn a_circles_rim_dragged_past_a_kilometre_keeps_the_begin_state_and_says_why() {
    // ph000002 on the circle about the origin, held there, dragged to (1e6, 1e6): the
    // radius would be √2 km, past the kilometre a radius may be. The begin state is kept,
    // and why is said, naming the sketch.
    let doc = document(CIRCLE_HELD);
    let solvers = solvers();
    let ended = dragged(&solvers, &doc, "ph000002", &[], [1e6, 1e6]);
    check_end(&doc, &ended, false, "rim past a kilometre");
    assert_eq!(ended.kept, Kept::Begin, "{:?}", ended.sketch);
}

#[test]
fn two_drags_open_at_once_on_one_pool_each_end_as_alone() {
    // Each drag holds a worker of its own; their ends settle on whichever is idle.
    let (line, arc) = (document(LINE), document(ARC));
    let solvers = solvers();
    let a = begin(
        &solvers,
        None,
        &line,
        &only_feature(&line),
        &id("pf000002"),
        None,
    )
    .unwrap();
    let b = begin(
        &solvers,
        None,
        &arc,
        &only_feature(&arc),
        &id("pc000003"),
        None,
    )
    .unwrap();
    a.drag.move_to([4.0, 4.0]).unwrap().wait().unwrap();
    b.drag.move_to([2.0, 9.0]).unwrap().wait().unwrap();
    let ended_b = b.drag.end(Some([2.0, 9.0])).unwrap();
    let ended_a = a.drag.end(Some([4.0, 4.0])).unwrap();
    let alone_a = dragged(&solvers, &line, "pf000002", &[], [4.0, 4.0]);
    let alone_b = dragged(&solvers, &arc, "pc000003", &[], [2.0, 9.0]);
    assert_eq!(ended_a, alone_a);
    assert_eq!(ended_b, alone_b);
}

// ---------------------------------------------------------------------------------------
// Property: every end keeps its promises, and depends on the begin state and the final
// cursor alone.

/// Each sketch, and the points dragged in it.
const CASES: [(&str, &[&str]); 8] = [
    (
        PROFILE,
        &[
            "pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "pa000006",
        ],
    ),
    (ARC, &["pc000001", "pc000002", "pc000003"]),
    (
        TWO_ARCS,
        &["pd000001", "pd000002", "pd000003", "pd000004", "pd000005"],
    ),
    (CIRCLE, &["pe000001", "pe000002"]),
    (LINE, &["pf000001", "pf000002"]),
    (CIRCLE_HELD, &["ph000001", "ph000002"]),
    (SYMMETRIC, &["pq000001", "pq000002", "pq000003"]),
    (FREE_ARC, &["pr000001", "pr000002", "pr000003"]),
];

/// A coordinate near `base` or at an edge: a hair off it, a few millimetres off, exactly
/// another of the sketch's coordinates, zero of either sign, or at the kilometre's reach.
fn coordinate() -> impl Strategy<Value = (u8, f64)> {
    (0u8..8, -1.0f64..1.0)
}

fn place(kind: u8, r: f64, base: f64, other: f64) -> f64 {
    match kind {
        0 => base + r * 1e-9,
        1 => base + r * 1e-12,
        2 => base + r * 20.0,
        3 => other,
        4 => {
            if r < 0.0 {
                -0.0
            } else {
                0.0
            }
        }
        5 => 1e6 * r.signum(),
        6 => (1e6 - 1e-3) * r.signum(),
        _ => base + r * 1e3,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn every_end_passes_the_gate_has_no_drift_and_is_the_final_cursors_alone(
        which in 0usize..CASES.len(),
        point in 0usize..6,
        other in 0usize..6,
        x in coordinate(),
        y in coordinate(),
        detour in (-50.0f64..50.0, -50.0f64..50.0),
    ) {
        thread_local! {
            static SOLVERS: SketchSolvers = solvers();
        }
        let (text, points) = CASES[which];
        let doc = document(text);
        let file = sketch_of(&doc);
        let p = points[point % points.len()];
        let o = points[other % points.len()];
        let base = point_of(&file, p);
        let near = point_of(&file, o);
        let to = [place(x.0, x.1, base[0], near[0]), place(y.0, y.1, base[1], near[1])];
        let what = format!("{p} of {} to {to:?}", only_feature(&doc));
        SOLVERS.with(|solvers| {
            // Straight to the cursor (the end's own move), and by way of a detour and the
            // cursor (the newest frame's values): the same end, bit for bit.
            let straight = dragged(solvers, &doc, p, &[], to);
            check_end(&doc, &straight, to == base, &what);
            let detour = [base[0] + detour.0, base[1] + detour.1];
            let winding = dragged(solvers, &doc, p, &[detour, to], to);
            assert_eq!(&winding, &straight, "{what}, by way of {detour:?}");
        });
    }
}

#[test]
fn a_free_circles_centre_or_rim_dragged_across_the_other_ends_keeping_the_begin_state_saying_why() {
    // Found by break-it, fixed: the free circle about pe000001 (0, 0) through pe000002
    // (3, 0). Its centre dragged to (9.718434187344899, 0), past the rim point, or its rim
    // point dragged to (-16.4597038764098, 0), across the centre: SQP shrinks the radius to
    // about 1e-15 and leaves the centre and pe000002 at one place, the cursor far off. The
    // worker passes that frame (a radius above 0 builds), but the end's whole-sketch solve,
    // sent from it, failed the system's own check (a point held on a circle drawn on its
    // centre, `SketchSystem::check`'s `apart`), and `end` was an `Err` in the solver's
    // indices. Such a frame is `Invalid` now, and an end at it keeps the begin state,
    // saying why (unnamed: the worker withholds an invalid frame's values). The property
    // below found both inputs.
    let doc = document(CIRCLE);
    let solvers = solvers();
    for (point, to) in [
        ("pe000001", [9.718434187344899, 0.0]),
        ("pe000002", [-16.4597038764098, 0.0]),
    ] {
        let Opened { drag, .. } =
            begin(&solvers, None, &doc, &only_feature(&doc), &id(point), None).unwrap();
        let ended = drag
            .end(Some(to))
            .unwrap_or_else(|e| panic!("{point} to {to:?}: the end is refused: {e:?}"));
        assert_eq!((ended.kept, ended.changed), (Kept::Begin, false), "{point}");
        assert_eq!(ended.sketch, sketch_of(&doc));
        let why = ended.why.unwrap();
        assert_eq!(why.feature(), Some("s0000005"));
        assert_eq!(why.code(), ErrorCode::InvalidResult, "{point}: {why:?}");
        assert!(
            why.message()
                .contains("a point held on a circle drawn at its centre"),
            "{point}: {why:?}"
        );
    }
}

#[test]
fn a_point_dragged_onto_the_centre_its_symmetric_partner_is_held_about_keeps_the_begin_state() {
    // pq000001 and pq000002 symmetric about pq000003, held at the origin; pq000001 dragged
    // onto the origin, where its partner would be too: the move doesn't solve (`Failed`,
    // as FreeCAD's), and the end keeps the begin state, saying why, naming the sketch.
    let doc = document(SYMMETRIC);
    let solvers = solvers();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &only_feature(&doc),
        &id("pq000001"),
        None,
    )
    .unwrap();
    let frame = drag.move_to([0.0, 0.0]).unwrap().wait().unwrap();
    assert_eq!(frame.kind, FrameKind::Failed, "{frame:?}");
    let ended = drag.end(Some([0.0, 0.0])).unwrap();
    assert_eq!((ended.kept, ended.changed), (Kept::Begin, false));
    check_end(&doc, &ended, false, "symmetric onto its centre");
}

#[test]
fn a_point_dragged_onto_another_a_reference_distance_measures_from_ends_without_an_error() {
    // ps000002 dragged exactly onto ps000001, a reference distance between them: either
    // the drag keeps the cursor and the sketch it leaves passes the gate, or it keeps the
    // begin state, saying why. Never an end refused in the solver's terms.
    let doc = document(DISTANCE_REF);
    let solvers = solvers();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &only_feature(&doc),
        &id("ps000002"),
        None,
    )
    .unwrap();
    let ended = drag
        .end(Some([0.0, 0.0]))
        .unwrap_or_else(|e| panic!("the end is refused: {e:?}"));
    check_end(&doc, &ended, false, "onto the reference's other point");
}

#[test]
fn a_frame_with_a_point_held_on_a_circle_drawn_on_its_centre_isnt_solved() {
    // Found by break-it, fixed: the same drag's frame was `Solved`, the circle's radius
    // 8.881784197001252e-16 and pe000002 exactly on its centre: geometry the next solve
    // refuses to start from (`SketchSystem::check`'s `apart`), shown to the user as the
    // drag's frame. A frame a solve can't start from is `Invalid` (M3.7: the client keeps
    // its last good frame).
    let doc = document(CIRCLE);
    let solvers = solvers();
    let Opened { drag, .. } = begin(
        &solvers,
        None,
        &doc,
        &only_feature(&doc),
        &id("pe000001"),
        None,
    )
    .unwrap();
    let frame = drag
        .move_to([9.718434187344899, 0.0])
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(frame.kind, FrameKind::Invalid, "{frame:?}");
}
