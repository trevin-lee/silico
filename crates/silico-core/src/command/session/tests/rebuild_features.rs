//! `status.rebuild.features` (issue #367, Fable's decision of 2026-11-04): each unsuppressed
//! feature's state in the session's rebuild, in timeline order, from its start, kept once it
//! ends. Expected values are worked by hand from each document's features and the steps
//! they make, the fake kernel holding one request so a rebuild is read part way.

use super::*;
use crate::recompute::FeatureStates;
use StepState::{Blocked, Built, Failed, Faulted, Queued, Running};

/// A feature's prefix for the documents below.
const HEAD: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

fn doc(features: &[&str]) -> String {
    format!("{HEAD}{}]}}", features.join(", "))
}

const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
const PIN: &str = r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#;
const HOLE: &str = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "c0000001"}"#;
const BALL: &str = r#"{"id": "q0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#;
const BOTH: &str = r#"{"id": "w0000001", "type": "fuse", "v": 1, "name": "Both", "base": "m3tq8v0z", "tool": "q0000001"}"#;

/// Two independent primitives, the block and the pin, and the cut of one by the other.
fn parts() -> String {
    doc(&[BLOCK, PIN, HOLE])
}

/// [`parts`], with a ball on a branch of its own and the fuse of the cut and the ball: the
/// ball, held, keeps a rebuild running while the rest settles.
fn parts_and_ball() -> String {
    doc(&[BLOCK, PIN, HOLE, BALL, BOTH])
}

fn is_sphere(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Sphere { .. }))
}

/// The session's rebuild's progress, as `status` gives it.
fn progress(f: &Fixture) -> Option<Progress> {
    f.sessions.status(&f.id).result.unwrap().rebuild.progress
}

/// The session's rebuild's features, as `status` gives them.
fn features(f: &Fixture) -> Option<FeatureStates> {
    progress(f).map(|progress| progress.features)
}

/// Polls `status` until its rebuild's features are `want`, for at most [`WAIT`].
#[track_caller]
fn until_features(f: &Fixture, want: &[(&str, StepState)]) {
    let want = states(want);
    let started = std::time::Instant::now();
    while features(f).as_ref() != Some(&want) {
        assert!(started.elapsed() < WAIT, "{:?}, not {want:?}", features(f));
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Polls `status` until its rebuild's state is `want`, for at most [`WAIT`].
#[track_caller]
fn until_state(f: &Fixture, want: RebuildState) {
    let started = std::time::Instant::now();
    while f.sessions.status(&f.id).result.unwrap().rebuild.state != want {
        assert!(started.elapsed() < WAIT, "never {want:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// No feature `queued` or `running`: what a finished rebuild leaves.
#[track_caller]
fn all_settled(features: &FeatureStates) {
    for (feature, state) in features.iter() {
        assert!(
            !matches!(state, Queued | Running),
            "{feature} is {state:?} after the rebuild finished"
        );
    }
}

#[test]
fn features_is_null_before_any_rebuild_of_the_revision() {
    // Opened, nothing has needed the model: no rebuild has started, so no list, as `done`
    // and `steps` have none; written `null`.
    let f = opened(&parts());
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.rebuild.progress, None);
    use crate::command::wire::ToWire;
    let wire =
        crate::command::wire::write(&status.to_wire(), crate::command::wire::Layout::OneLine)
            .unwrap();
    assert!(
        wire.contains(r#""steps": null, "features": null}"#),
        "{wire}"
    );
}

#[test]
fn while_one_primitive_is_held_the_other_is_built_and_the_cut_queued_then_all_are_built() {
    // The block at the origin (one step) and the pin at (20, 10, -1) (its cylinder and its
    // move: two) use nothing; the cut uses both. The pin's cylinder held: the block's step
    // settles, built; the pin is running; the cut waits on the pin, queued.
    let f = opened(&parts());
    let hold = f.kernel.hold(is_cylinder);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("k7f2qx3m", Built),
            ("c0000001", Running),
            ("m3tq8v0z", Queued),
        ],
    );
    let held = progress(&f).unwrap();
    assert_eq!((held.done, held.steps), (1, 4));
    hold.release();
    check.wait().result.unwrap();
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.rebuild.state, RebuildState::Done);
    let done = status.rebuild.progress.unwrap();
    assert_eq!(
        done.features,
        states(&[
            ("k7f2qx3m", Built),
            ("c0000001", Built),
            ("m3tq8v0z", Built)
        ])
    );
    all_settled(&done.features);
}

#[test]
fn a_refused_box_is_failed_and_what_uses_it_blocked_while_the_rebuild_runs_and_after() {
    // The block refused as the kernel refuses a box it can't make (a zero-length box, which
    // the gate refuses before any kernel, so the fake refuses this one in its place): failed,
    // its own problem. The cut uses it, blocked; the fuse uses the cut, blocked. The pin
    // builds; the ball, on a branch of its own, held, keeps the rebuild running meanwhile,
    // so the cut and the fuse are seen blocked as the block fails, not only at the end.
    let f = opened(&parts_and_ball());
    f.kernel.refuse(|op| {
        is_box(op).then(|| {
            Error::new(ErrorCode::KernelFailed, "MakeBox failed").with_cause("a box of zero length")
        })
    });
    let hold = f.kernel.hold(is_sphere);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("k7f2qx3m", Failed),
            ("c0000001", Built),
            ("m3tq8v0z", Blocked),
            ("q0000001", Running),
            ("w0000001", Blocked),
        ],
    );
    assert_eq!(
        f.sessions.status(&f.id).result.unwrap().rebuild.state,
        RebuildState::Running
    );
    hold.release();
    let _ = check.wait();
    let status = f.sessions.status(&f.id).result.unwrap();
    // A failure is the document's, not a fault.
    assert_eq!(status.rebuild.state, RebuildState::Done);
    let done = status.rebuild.progress.unwrap().features;
    assert_eq!(
        done,
        states(&[
            ("k7f2qx3m", Failed),
            ("c0000001", Built),
            ("m3tq8v0z", Blocked),
            ("q0000001", Built),
            ("w0000001", Blocked),
        ])
    );
    all_settled(&done);
}

#[test]
fn a_crash_is_faulted_then_what_uses_it_blocked() {
    // As above, but the block's worker crashes: a kernel fault, not the document's
    // problem, so the block is faulted, mirroring the rebuild's own `faulted`; what uses it
    // is blocked, as for a failure.
    let f = opened(&parts_and_ball());
    f.kernel
        .refuse(|op| is_box(op).then(|| Error::new(ErrorCode::WorkerCrashed, "crashed")));
    let hold = f.kernel.hold(is_sphere);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("k7f2qx3m", Faulted),
            ("c0000001", Built),
            ("m3tq8v0z", Blocked),
            ("q0000001", Running),
            ("w0000001", Blocked),
        ],
    );
    hold.release();
    let _ = check.wait();
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.rebuild.state, RebuildState::Faulted);
    let done = status.rebuild.progress.unwrap().features;
    assert_eq!(
        done,
        states(&[
            ("k7f2qx3m", Faulted),
            ("c0000001", Built),
            ("m3tq8v0z", Blocked),
            ("q0000001", Built),
            ("w0000001", Blocked),
        ])
    );
    all_settled(&done);
}

#[test]
fn a_cancelled_rebuild_is_stopped_its_built_features_built_and_the_rest_queued() {
    // The pin's cylinder held, the block built; Stop. No `cancelled` state: the rebuild
    // says `stopped`, the block stays built, and the pin (running when stopped) and the cut
    // are queued, for the next rebuild to resume.
    let f = opened(&parts());
    let hold = f.kernel.hold(is_cylinder);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("k7f2qx3m", Built),
            ("c0000001", Running),
            ("m3tq8v0z", Queued),
        ],
    );
    assert!(
        f.sessions
            .cancel(&f.id, Cancellable::Rebuild)
            .result
            .unwrap()
            .stopped
    );
    let _ = check.wait();
    until_state(&f, RebuildState::Stopped);
    // Said stopped, nothing is said running, whether or not its recompute has ended yet.
    let stopped = states(&[
        ("k7f2qx3m", Built),
        ("c0000001", Queued),
        ("m3tq8v0z", Queued),
    ]);
    assert_eq!(features(&f).unwrap(), stopped);
    // Once its recompute has ended (the held cylinder superseded), its own count says the
    // same, and so does status, for good.
    let counter = f.session().newest.counter().unwrap();
    let mut seen = crate::recompute::StepCount::default();
    while !seen.ended {
        seen = counter.changed_since(&seen);
    }
    assert_eq!(seen.features, stopped);
    hold.release();
    assert!(f.kernel.wait_idle(WAIT));
    let status = f.sessions.status(&f.id).result.unwrap();
    assert_eq!(status.rebuild.state, RebuildState::Stopped);
    assert_eq!(status.rebuild.progress.unwrap().features, stopped);
}

#[test]
fn a_suppressed_feature_is_absent_and_the_order_is_the_timelines() {
    // The timeline's order isn't its IDs' (z, a, m) nor its steps': the pin z0000001
    // first, the block a0000001, a ball suppressed between them and the cut, and the cut
    // m0000001 of the block by the pin last. The ball has no step: it isn't listed.
    let text = doc(&[
        r#"{"id": "z0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#,
        r#"{"id": "a0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#,
        r#"{"id": "b0000001", "type": "sphere", "v": 1, "name": "Ball", "suppressed": true, "position": [0, 0, 0], "radius": 5}"#,
        r#"{"id": "m0000001", "type": "cut", "v": 1, "name": "Hole", "base": "a0000001", "tool": "z0000001"}"#,
    ]);
    let f = opened(&text);
    let hold = f.kernel.hold(is_cylinder);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("z0000001", Running),
            ("a0000001", Built),
            ("m0000001", Queued),
        ],
    );
    hold.release();
    check.wait().result.unwrap();
    let done = features(&f).unwrap();
    let listed: Vec<&str> = done.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(listed, ["z0000001", "a0000001", "m0000001"]);
    assert_eq!(done.get(&id("b0000001")), None);
    assert_eq!(
        done,
        states(&[
            ("z0000001", Built),
            ("a0000001", Built),
            ("m0000001", Built)
        ])
    );
}

/// A square sketch `s0000001` on xy, its corners (0, 0) to (10, 10), with no constraint.
const SQUARE: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Square", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}, {"id": "pa000003", "x": 10, "y": 10, "construction": false}, {"id": "pa000004", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
const PAD: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;

#[test]
fn a_swept_sketch_is_running_while_it_solves_then_built() {
    // The square, padded, is solved before the plan (the pad's steps depend on its
    // profile): held in its solve, it's running, and the pad and the block, planned after,
    // are queued. Released, all three are built.
    let sketches = crate::fake::FakeSketches::new();
    sketches.hold();
    let f = opened_solving(&doc(&[SQUARE, PAD, BLOCK]), &sketches);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(sketches.wait_held(WAIT));
    until_features(
        &f,
        &[
            ("s0000001", Running),
            ("p0000001", Queued),
            ("k7f2qx3m", Queued),
        ],
    );
    sketches.release();
    check.wait().result.unwrap();
    let done = features(&f).unwrap();
    assert_eq!(
        done,
        states(&[
            ("s0000001", Built),
            ("p0000001", Built),
            ("k7f2qx3m", Built)
        ])
    );
}

#[test]
fn a_sketch_nothing_sweeps_is_running_while_it_solves_beside_the_steps_then_built() {
    // The square alone, no pad: solved in the first round, beside the block's step, which
    // needs nothing of it. Held in its solve, it's running while the block is built.
    let sketches = crate::fake::FakeSketches::new();
    sketches.hold();
    let f = opened_solving(&doc(&[SQUARE, BLOCK]), &sketches);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(sketches.wait_held(WAIT));
    until_features(&f, &[("s0000001", Running), ("k7f2qx3m", Built)]);
    sketches.release();
    check.wait().result.unwrap();
    assert_eq!(
        features(&f).unwrap(),
        states(&[("s0000001", Built), ("k7f2qx3m", Built)])
    );
}

#[test]
fn a_sketch_that_doesnt_solve_is_failed_and_its_pad_blocked() {
    // The solver refuses the square as conflicting: the sketch is failed, its own problem,
    // and the pad that sweeps it blocked; the block builds.
    let sketches = crate::fake::FakeSketches::new();
    sketches.answer(|_| {
        Err(Error::new(ErrorCode::SolveFailed, "the sketch didn't solve").with_feature("s0000001"))
    });
    let f = opened_solving(&doc(&[SQUARE, PAD, BLOCK]), &sketches);
    let _ = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    let done = features(&f).unwrap();
    assert_eq!(
        done,
        states(&[
            ("s0000001", Failed),
            ("p0000001", Blocked),
            ("k7f2qx3m", Built)
        ])
    );
    all_settled(&done);
}

#[test]
fn a_pad_whose_faces_and_prism_are_made_waits_queued_for_its_base() {
    // The square padded onto the block, the block held. The pad's faces and its prism need
    // nothing of the block, and are made; its fuse waits on the block. No step of the pad's
    // runs then, so it's queued, not running (a step it owns running is what running says),
    // until the block is let go; then all are built.
    let on_block = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "base": "k7f2qx3m", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let f = opened(&doc(&[SQUARE, BLOCK, on_block]));
    let hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    // The faces and the prism settled: 2 done of 4 (the block, the faces, the prism, the
    // fuse).
    let started = std::time::Instant::now();
    while progress(&f).is_none_or(|p| p.done < 2) {
        assert!(started.elapsed() < WAIT, "{:?}", progress(&f));
        std::thread::sleep(Duration::from_millis(1));
    }
    until_features(
        &f,
        &[
            ("s0000001", Built),
            ("k7f2qx3m", Running),
            ("p0000001", Queued),
        ],
    );
    assert_eq!(progress(&f).map(|p| (p.done, p.steps)), Some((2, 4)));
    hold.release();
    check.wait().result.unwrap();
    assert_eq!(
        features(&f).unwrap(),
        states(&[
            ("s0000001", Built),
            ("k7f2qx3m", Built),
            ("p0000001", Built)
        ])
    );
}

#[test]
fn a_fillet_whose_edge_names_nothing_is_failed_and_its_user_blocked_while_the_rebuild_runs() {
    // The block's fillet names an edge the box hasn't (a box has 12): its reference is
    // broken, its own problem, so failed; a cut of the fillet by the ball is blocked on it.
    // The ball held keeps the rebuild running: both are seen before it ends.
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge99"], "radius": 1}"#;
    let cut = r#"{"id": "j0000001", "type": "cut", "v": 1, "name": "Cut", "base": "f0000001", "tool": "q0000001"}"#;
    let f = opened(&doc(&[BLOCK, fillet, BALL, cut]));
    let hold = f.kernel.hold(is_sphere);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    until_features(
        &f,
        &[
            ("k7f2qx3m", Built),
            ("f0000001", Failed),
            ("q0000001", Running),
            ("j0000001", Blocked),
        ],
    );
    hold.release();
    let _ = check.wait();
    let done = features(&f).unwrap();
    assert_eq!(
        done,
        states(&[
            ("k7f2qx3m", Built),
            ("f0000001", Failed),
            ("q0000001", Built),
            ("j0000001", Blocked),
        ])
    );
    all_settled(&done);
}
