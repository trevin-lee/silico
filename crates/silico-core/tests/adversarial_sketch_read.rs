//! break-it on the `sketch` read (`Sessions::sketch`, `command/sketch.rs`) on the fake
//! sketch solver: published reads at once against the design's bound on sketch workers, a
//! sketch whose rebuild solve failed read against `check` of the same model, a sketch
//! removed in an open transaction read from the current and the published model, and a
//! document the gate refuses.
//! Expected values: docs/design/m5.0-app.md (section 6's *Per-client generations*, section
//! 7's `sketch`), the result schema's own words (`solve_problem`: "as `check` reports it"),
//! and the counts sent here.

use crate::sessions::{HANG, answer, id};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, MAX_PUBLISHED_READS, ModelChoice, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};

/// A sketch `k7f2qx3m` on xy: a free line.
const SKETCH: &str = r#"{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;

/// A second sketch, `m7f2qx3m`, empty.
const OTHER: &str = r#"{"id": "m7f2qx3m", "type": "sketch", "v": 1, "name": "T", "plane": "xz", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// Sessions on a fake kernel, every sketch solved by `start`'s solver, and `text` open in
/// the first, its rebuild checked.
fn opened(
    dir: &tempfile::TempDir,
    text: &str,
    start: impl Fn() -> Result<Box<dyn SketchSolver>, Error> + Send + Sync + 'static,
) -> Sessions<Arc<FakeKernel>> {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &std::path::Path| Cache::unsealed(cache),
        start,
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let opened = answer(sessions.open(path, false));
    assert!(opened.result.is_ok(), "{:?}", opened.result);
    answer(sessions.check(
        &opened.session.unwrap().id,
        silico_core::command::CheckModel::Current { fresh: false },
    ));
    sessions
}

fn s1() -> silico_core::command::SessionId {
    silico_core::command::SessionId::parse("s1").unwrap()
}

/// The envelope `name` answers `arguments` with, through the sessions' call by name.
fn call(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Value {
    match sessions.call(name, arguments) {
        Call::Running(calling) => {
            let answered = calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}"));
            serde_json::from_str(&answered.text).unwrap()
        }
        Call::Unknown => panic!("no command {name}"),
    }
}

#[test]
fn published_sketch_reads_at_once_start_no_more_sketch_workers_than_a_session_may_hold() {
    // Found by break-it: each published `sketch` read took a solver of the sessions' shared
    // ones for a solve of its own, and with none idle `SketchSolvers::take` started another,
    // unbounded, each kept idle afterwards: one session's eight reads at once started eight.
    // The design bounds them (section 6, *Per-client generations*: "as many sketch workers
    // can live as sessions were solving and dragging at once: 32 at most, one recompute's
    // and one drag's for each of 16 sessions, never more"; A8: `sketch` "needs none"). The
    // model now keeps each sketch's diagnosis: the reads, the solver held so a solve would
    // hang, start none and ask nothing, and answer.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let starts = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&starts);
    let sessions = opened(&dir, &document(&[SKETCH]), move || {
        started.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    assert_eq!(starts.load(Ordering::SeqCst), 1, "the rebuild's solver");
    let asked = sketches.requests().len();
    sketches.hold();
    let reads: Vec<_> = (0..MAX_PUBLISHED_READS)
        .map(|_| sessions.sketch(&s1(), ModelChoice::Published, id("k7f2qx3m")))
        .collect();
    for read in reads {
        let read = answer(read);
        assert!(read.result.is_ok(), "{:?}", read.result);
    }
    // Every read answered: any start one asked for has been counted.
    let at_once = starts.load(Ordering::SeqCst);
    sketches.release();
    assert_eq!(
        (at_once, sketches.requests().len()),
        (1, asked),
        "one session's {MAX_PUBLISHED_READS} published sketch reads started a sketch solver, \
         or asked one"
    );
}

#[test]
fn a_sketch_whose_rebuild_solve_failed_reads_as_check_reports_it() {
    // Found by break-it: the model's solve of the sketch failed (the worker crashed under
    // the rebuild), and `check` of that model reports it, the model's fault naming the
    // sketch; the read solved the sketch again and answered it solved, `solve_problem` null,
    // from a solve the model never had (the schema's `solve_problem` is "why the sketch
    // wasn't solved, as `check` reports it"). The read now gives what the rebuild found: the
    // rebuild's failure, as its refusal, the model's fault itself, worded as `elements`
    // words a fault that failed its feature.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let first = Arc::new(AtomicBool::new(true));
    let fails = Arc::clone(&first);
    sketches.answer(move |system| {
        if fails.swap(false, Ordering::SeqCst) {
            Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch worker crashed",
            ))
        } else {
            Ok(FakeSketches::as_sent(system))
        }
    });
    let solver = sketches.clone();
    let sessions = opened(&dir, &document(&[SKETCH]), move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let checked = call(
        &sessions,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    // The rebuild's failure is the model's fault, the sketch named: check reports it so.
    let fault = &checked["model"]["fault"];
    assert_eq!(
        (&fault["code"], &fault["feature"]),
        (&json!("worker_crashed"), &json!("k7f2qx3m")),
        "{checked}"
    );
    let read = call(
        &sessions,
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#,
    );
    assert_eq!(
        read["model"], checked["model"],
        "the same model, its fault and all"
    );
    assert_eq!(
        (&read["ok"], &read["errors"], &read["total"]),
        (&json!(false), &json!([fault]), &json!(1)),
        "{read}"
    );
}

#[test]
fn a_sketch_removed_in_a_transaction_is_refused_naming_the_sketches_left() {
    // The current model's document hasn't the sketch: refused naming `sketch`, the one
    // sketch left the candidates.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = opened(&dir, &document(&[SKETCH, OTHER]), move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let read = |model: &str| {
        call(
            &sessions,
            "sketch",
            &json!({"session": "s1", "model": model, "sketch": "k7f2qx3m"}).to_string(),
        )
    };
    let before = read("current");
    assert_eq!(before["ok"], true, "{before}");
    assert!(call(&sessions, "begin", r#"{"session": "s1"}"#)["ok"] == true);
    let removed = call(
        &sessions,
        "remove_feature",
        r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m"}"#,
    );
    assert_eq!(removed["ok"], true, "{removed}");
    let current = read("current");
    assert_eq!(current["ok"], false, "{current}");
    let error = &current["errors"][0];
    assert_eq!(
        (
            &error["code"],
            &error["parameter"],
            &error["message"],
            &error["candidates"]
        ),
        (
            &json!("invalid_input"),
            &json!("sketch"),
            &json!("the document has no feature k7f2qx3m"),
            &json!(["m7f2qx3m"])
        )
    );
    // The edit's rebuild needs no kernel (sketches only), so the newest published model
    // is revision 2's, which hasn't it either: refused alike, naming that model.
    let published = read("published");
    assert_eq!(published["model"]["revision"], 2, "{published}");
    assert_eq!(published["errors"], current["errors"]);
}

#[test]
fn a_sketch_read_of_a_document_the_gate_refuses_gives_the_gates_problems() {
    // A line naming a point the sketch hasn't: the gate refuses the document, so there's no
    // model, and the read answers the gate's problems, the solver never asked.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let broken = SKETCH.replace(r#""to": "p0000002""#, r#""to": "p0000009""#);
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &std::path::Path| Cache::unsealed(cache),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("broken.silico");
    std::fs::write(&path, document(&[&broken])).unwrap();
    let opened = answer(sessions.open(path, false));
    assert!(opened.result.is_ok(), "{:?}", opened.result);
    for model in ["current", "published"] {
        let read = call(
            &sessions,
            "sketch",
            &json!({"session": "s1", "model": model, "sketch": "k7f2qx3m"}).to_string(),
        );
        assert_eq!(read["ok"], false, "{read}");
        let error = &read["errors"][0];
        assert!(
            error["elements"]
                .as_array()
                .is_some_and(|e| e.iter().any(|e| e == "n0000001" || e == "p0000009"))
                || error["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("p0000009")),
            "the gate's problem names the line or the point it names: {read}"
        );
    }
    assert!(sketches.requests().is_empty(), "no solve asked");
}

/// A sketch `k7f2qx3m` on xy of `n` lines apart, each from (10i, 0) to (10i, 5), none
/// joined: each line's two ends open, so 2n open-end refusals (M3.5's third check), in
/// point order.
fn lines_apart(n: usize) -> String {
    let points: Vec<String> = (0..n)
        .flat_map(|i| {
            [
                format!(
                    r#"{{"id": "p{:07}", "x": {}, "y": 0, "construction": false}}"#,
                    2 * i + 1,
                    10 * i
                ),
                format!(
                    r#"{{"id": "p{:07}", "x": {}, "y": 5, "construction": false}}"#,
                    2 * i + 2,
                    10 * i
                ),
            ]
        })
        .collect();
    let lines: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "n{:07}", "from": "p{:07}", "to": "p{:07}", "construction": false}}"#,
                i + 1,
                2 * i + 1,
                2 * i + 2
            )
        })
        .collect();
    format!(
        r#"{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

#[test]
fn two_hundred_and_two_profile_refusals_list_a_hundred_and_count_every_one_as_check_does() {
    // Found by break-it: the read copied the profile's first hundred refusals and said
    // nothing of the rest, though `check` counts them. 101 lines apart: 202 open ends, each
    // refused (M3.5, by hand: two ends a line, none shared). The read lists the first 100
    // (invariant 9), the first p0000001's, the last p0000100's, in point order, and counts
    // 202; `check` of a pad sweeping the sketch counts the same 202 in its words.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let pad = r#"{"id": "d0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "k7f2qx3m", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let sessions = opened(&dir, &document(&[&lines_apart(101), pad]), move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let read = call(
        &sessions,
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#,
    );
    assert_eq!(read["ok"], true, "{read}");
    let result = &read["result"];
    let listed = result["profile_problems"].as_array().unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(result["profile_problems_total"], 202);
    assert_eq!(listed[0]["elements"], json!(["p0000001", "n0000001"]));
    assert_eq!(listed[99]["elements"], json!(["p0000100", "n0000050"]));
    assert_eq!(result["regions"], json!([]));
    let checked = call(
        &sessions,
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    // `check` words the pad's one failure with the count, and its cause's tail the rest.
    let problem = &checked["result"]["problems"][0];
    assert_eq!(
        problem["message"],
        "pad d0000001 sweeps sketch k7f2qx3m, which makes no profile (202 refusals)",
        "{checked}"
    );
    assert!(
        problem["cause"]
            .as_str()
            .is_some_and(|c| c.ends_with("; and 102 more")),
        "{checked}"
    );
}

#[test]
fn more_than_a_hundred_dependent_groups_and_a_group_of_more_than_a_hundred_list_a_hundred_and_count_all()
 {
    // 101 lines apart, each held horizontal: 101 constraints, k0000001 to k0000101, their
    // tags in ID order. The solver (the fake, told so) finds 102 groups: each constraint
    // alone, and all 101 together, in lexicographic order: [k1], [k1 .. k101], [k2], ...,
    // [k101]. The read lists the first 100 groups (invariant 9), the second its first 100
    // constraints with its total, 101, and counts 102 groups.
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let mut tags: Vec<_> = system.constraints.iter().filter_map(|c| c.tag()).collect();
        tags.sort_unstable();
        let mut groups: Vec<Vec<_>> = tags.iter().map(|&t| vec![t]).collect();
        groups.push(tags.clone());
        groups.sort();
        let mut solved = FakeSketches::as_sent(system);
        solved.dependencies = groups;
        Ok(solved)
    });
    let solver = sketches.clone();
    let horizontals: Vec<String> = (1..=101)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "n{i:07}"}}"#))
        .collect();
    let sketch = lines_apart(101).replace(
        r#""constraints": []"#,
        &format!(r#""constraints": [{}]"#, horizontals.join(", ")),
    );
    let sessions = opened(&dir, &document(&[&sketch]), move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let read = call(
        &sessions,
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#,
    );
    assert_eq!(read["ok"], true, "{read}");
    let result = &read["result"];
    assert_eq!(result["dependencies_total"], 102);
    let groups = result["dependencies"].as_array().unwrap();
    assert_eq!(groups.len(), 100);
    let k = |i: usize| format!("k{i:07}");
    assert_eq!(
        groups[0],
        json!({"constraints": [k(1)], "constraints_total": 1})
    );
    let all: Vec<String> = (1..=100).map(k).collect();
    assert_eq!(
        groups[1],
        json!({"constraints": all, "constraints_total": 101})
    );
    for (i, group) in groups.iter().enumerate().skip(2) {
        assert_eq!(
            *group,
            json!({"constraints": [k(i)], "constraints_total": 1}),
            "group {i}"
        );
    }
}

#[test]
fn negative_degrees_of_freedom_are_written_as_the_solver_gave_them() {
    // PlaneGCS's number as given (the design's `dofs`): the fake answers −1, as PlaneGCS
    // does for an arc whose rules alone repeat each other. The writer gives −1 exactly, not
    // its magnitude, nor a count clamped at 0. Nothing flagged and a solution: `state` by
    // the design's rule (`dofs` above 0 under_defined, any other fully_defined).
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.dofs = -1;
        Ok(solved)
    });
    let solver = sketches.clone();
    let sessions = opened(&dir, &document(&[SKETCH]), move || {
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    let arguments = r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m"}"#;
    let Call::Running(calling) = sessions.call("sketch", arguments) else {
        panic!("no command sketch");
    };
    let text = calling
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("sketch didn't answer in {HANG:?}"))
        .text;
    assert_eq!(text.matches(r#""dofs": -1,"#).count(), 1, "{text}");
    let read: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (&read["result"]["dofs"], &read["result"]["state"]),
        (&json!(-1), &json!("fully_defined")),
        "{read}"
    );
}
