//! `sketch_add` and `sketch_remove` on PlaneGCS (docs/design/m5.0-app.md, section 7, *The
//! sketch edits*, and section 13's SD7 tests): through the sessions' own command called by
//! name, as a transport calls it (`Sessions::call`), on the fake kernel, each sketch solved
//! by this crate's worker. The rule they hold to, SD7: an edit the gate takes is applied,
//! never refused for what the sketch's solve then finds, which is the sketch's state.
//!
//! Expected values by hand: the bracket's L profile is fully defined, so a second length on
//! its base line, `na000001`, at 61 mm where `ka000009` holds it at 60, conflicts (two
//! driving lengths of one line; FreeCAD's solver reports the pair, docs/ports/sketcher.md);
//! made driven, it measures the line at 60 and conflicts with nothing. Removing a member
//! takes equations away and so never makes a solvable sketch unsolvable: removing the base
//! leg's thickness, `ka00000a`, leaves one freedom (that thickness) and a profile that still
//! closes; removing the line `na000003` leaves the profile open, which the pad fails on.

use super::bracket::{P0000001, S0000001, S0000002};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Revision, SessionId, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// The second length of the L profile's base line, 61 mm where `ka000009` holds 60: what
/// section 13's SD7 test adds.
const SECOND_LENGTH: &str =
    r#"{"id": "ka00000d", "type": "length", "line": "na000001", "value": 61, "driving": true}"#;

/// Sessions on a fake kernel, each sketch solved by the sketch worker, with the bracket's
/// L profile, its pad, and the base's holes open as s1 from `bracket.silico`.
struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
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
    let f = Fixture { dir, sessions };
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [S0000001, P0000001, S0000002].join(", ")
    );
    std::fs::write(f.path(), text).unwrap();
    let opened = f.call(
        "open",
        &json!({"path": f.path().to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    f
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    /// The envelope the command `name` answers to `arguments`, as a transport has it.
    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer: Answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        let envelope: Value = serde_json::from_str(&answer.text).unwrap();
        assert_eq!(envelope["ok"], answer.ok, "{}", answer.text);
        envelope
    }

    /// The result of a command that ran.
    fn ran(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }

    fn revision(&self) -> u64 {
        self.sessions
            .status(&s1())
            .session
            .unwrap()
            .revision
            .number()
    }

    fn text(&self) -> String {
        self.ran("text", r#"{"session": "s1", "feature": null}"#)["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The current model's problems, by code and feature, in order.
    fn problems(&self) -> Vec<(String, Option<String>)> {
        let checked = self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        listed(&checked["problems"])
    }

    /// The current model's first problem of `feature`.
    fn problem_of(&self, feature: &str) -> Value {
        let checked = self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        checked["problems"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["feature"] == feature)
            .cloned()
            .unwrap_or_else(|| panic!("no problem of {feature}: {checked}"))
    }

    /// `sketch_add` of `constraints` (each a constraint's JSON) to the L profile, at the
    /// session's revision.
    fn add_constraints(&self, constraints: &str) -> Value {
        self.call(
            "sketch_add",
            &format!(
                r#"{{"session": "s1", "revision": {}, "sketch": "s0000001", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": [{constraints}]}}"#,
                self.revision()
            ),
        )
    }

    /// `sketch_remove` of the L profile's `members`, at the session's revision.
    fn remove(&self, members: &[&str]) -> Value {
        self.call(
            "sketch_remove",
            &json!({"session": "s1", "revision": self.revision(), "sketch": "s0000001", "members": members})
                .to_string(),
        )
    }

    fn undo(&self) {
        let at = revision(self.revision());
        let undone = self.sessions.undo(&s1(), at, None).wait_timeout(HANG);
        assert!(undone.is_ok_and(|r| r.result.is_ok()), "undo at {at:?}");
    }

    fn redo(&self) {
        let at = revision(self.revision());
        let redone = self.sessions.redo(&s1(), at, None).wait_timeout(HANG);
        assert!(redone.is_ok_and(|r| r.result.is_ok()), "redo at {at:?}");
    }
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

/// Each problem's code and feature, in order.
fn listed(problems: &Value) -> Vec<(String, Option<String>)> {
    problems
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["code"].as_str().unwrap().to_owned(),
                p["feature"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// The constraints' IDs of the L profile as `text`, a document, holds it.
fn constraints_of(text: &str) -> Vec<String> {
    let document: Value = serde_json::from_str(text).unwrap();
    document["features"][0]["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_dimension_that_over_defines_the_sketch_is_applied_and_its_conflict_reported() {
    let f = fixture();
    assert_eq!(f.problems(), []);
    f.ran("begin", r#"{"session": "s1"}"#);
    let opened = f.text();
    // Applied, as the gate takes it: the member added, at a new revision.
    let added = f.add_constraints(SECOND_LENGTH);
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(
        added["result"],
        json!({"sketch": "s0000001", "added": ["ka00000d"], "modified": [], "removed": [], "changed": true, "commit": null})
    );
    assert!(constraints_of(&f.text()).contains(&"ka00000d".to_owned()));
    // The rebuild's solve finds the conflict: the sketch fails, naming the two lengths, and
    // the pad on it with it, loudly.
    assert_eq!(
        f.problem_of("s0000001"),
        json!({
            "code": "over_constrained",
            "message": "the sketch's constraints conflict: these 2 can't all hold",
            "feature": "s0000001",
            "elements": ["ka000009", "ka00000d"],
            "elements_total": 2,
            "parameter": null,
            "cause": null,
            "suggestion": "remove or change one of the constraints named",
            "candidates": []
        })
    );
    // The one problem: what fails downstream is the pad, unbuilt, which a read of it says.
    assert_eq!(
        f.problems(),
        [("over_constrained".to_owned(), Some("s0000001".to_owned()))]
    );
    let pad = f.call(
        "mass_properties",
        r#"{"session": "s1", "model": "current", "feature": "p0000001"}"#,
    );
    assert_eq!(
        pad["errors"],
        json!([{
            "code": "invalid_input",
            "message": "p0000001 didn't build, since it uses s0000001, which failed",
            "feature": "p0000001",
            "elements": [],
            "elements_total": 0,
            "parameter": "feature",
            "cause": null,
            "suggestion": "mend s0000001 first: `check` says why it failed",
            "candidates": []
        }]),
        "{pad}"
    );
    // Undone, the text before it, byte for byte, and nothing fails.
    f.undo();
    assert_eq!(f.text(), opened);
    assert_eq!(f.problems(), []);
    // Redone, and made driven (SolidWorks' "driven"): the sketch solves, the reference
    // dimension measuring the line it no longer drives.
    f.redo();
    assert_eq!(f.problem_of("s0000001")["code"], "over_constrained");
    let set = f.ran(
        "set_field",
        &json!({"session": "s1", "revision": f.revision(), "feature": "s0000001", "field": "constraints.ka00000d.driving", "value": false})
            .to_string(),
    );
    assert_eq!(set["after"], false, "{set}");
    assert_eq!(f.problems(), []);
    // Committed, the file holds it, its value as the solve measured the line: 60.
    let committed = f.ran("commit", r#"{"session": "s1"}"#);
    assert_eq!(committed["written"], true, "{committed}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    let reference = file["features"][0]["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "ka00000d")
        .cloned()
        .unwrap();
    assert_eq!(
        reference,
        json!({"id": "ka00000d", "type": "length", "line": "na000001", "value": 60, "driving": false})
    );
}

#[test]
fn outside_a_transaction_the_over_defining_add_is_saved_with_its_conflict_said() {
    let f = fixture();
    let added = f.add_constraints(SECOND_LENGTH);
    assert_eq!(added["ok"], true, "{added}");
    let commit = &added["result"]["commit"];
    // Written, the conflict among what the commit couldn't fix.
    assert_eq!(commit["written"], true, "{commit}");
    let problems = listed(&commit["problems"]);
    assert!(
        problems.contains(&("over_constrained".to_owned(), Some("s0000001".to_owned()))),
        "{problems:?}"
    );
    let file = std::fs::read_to_string(f.path()).unwrap();
    assert!(constraints_of(&file).contains(&"ka00000d".to_owned()));
    // A sketch_remove on the sketch still over defined is applied too, the conflict still
    // there, and removing the second length mends it.
    let removed = f.remove(&["ka000003"]);
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(removed["result"]["removed"], json!(["ka000003"]));
    assert_eq!(f.problem_of("s0000001")["code"], "over_constrained");
    let removed = f.remove(&["ka00000d"]);
    assert_eq!(removed["ok"], true, "{removed}");
    assert!(
        !f.problems()
            .iter()
            .any(|(_, feature)| feature.as_deref() == Some("s0000001")),
    );
}

#[test]
fn a_removal_that_leaves_the_sketch_under_defined_or_open_is_applied() {
    let f = fixture();
    f.ran("begin", r#"{"session": "s1"}"#);
    let opened = f.text();
    // The base leg's thickness: one freedom left, the profile still closed, nothing fails.
    let removed = f.remove(&["ka00000a"]);
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(
        removed["result"],
        json!({"sketch": "s0000001", "added": [], "modified": [], "removed": ["ka00000a"], "changed": true, "commit": null})
    );
    assert_eq!(f.problems(), []);
    f.undo();
    assert_eq!(f.text(), opened);
    // The design's: the line na000003, which leaves the profile open. Applied, and the pad
    // on it fails, loudly; the sketch itself solves.
    let removed = f.remove(&["na000003"]);
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(
        removed["result"]["removed"],
        json!(["na000003", "ka000003"])
    );
    // The line's two ends, each now an end of one line alone (pa000003 of na000002,
    // pa000004 of na000004), as M3.5 says an open profile.
    assert_eq!(
        f.problems(),
        [("invalid_input".to_owned(), Some("p0000001".to_owned()))]
    );
    let open = f.problem_of("p0000001");
    assert_eq!(
        (&open["message"], &open["elements"], &open["parameter"]),
        (
            &json!("pad p0000001 sweeps sketch s0000001, which makes no profile (2 refusals)"),
            &json!(["na000002", "na000004", "pa000003", "pa000004"]),
            &json!("sketch")
        ),
        "{open}"
    );
    assert!(
        open["cause"].as_str().unwrap().starts_with(
            "point pa000003 ends line na000002 and no other profile curve, so the profile is open there"
        ),
        "{open}"
    );
    f.undo();
    assert_eq!(f.text(), opened);
    assert_eq!(f.problems(), []);
}
