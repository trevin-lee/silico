//! M4.2: a session rebuilds its document when a command first needs its model (decision
//! 13), on its own client of the shared kernel, so one session's rebuild never supersedes
//! another's (MA2); sessions in one directory share its cache; a revision is rebuilt once;
//! and dropping the sessions stops a rebuild rather than waiting for it. Expected values are
//! the fake kernel's call record, the box's analytic measures (40 × 20 × 10: volume 8000,
//! area 2800), and the limits as the design's table gives them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{self, ModelRead, Response, Revision, Running, SessionId, Sessions};
use silico_core::document::{FeatureId, Measure};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::lateness::{Lateness, allowance};

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// [`BOX`]'s box under another ID and name: the same steps, which are keyed by their values.
const SAME_BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b0000000", "type": "box", "v": 1, "name": "Other", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

const SPHERE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a91cw0de", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}]}"#;

/// Longer than any command here takes on a loaded machine: past it, a command hangs.
const HANG: Duration = Duration::from_secs(60);

/// Sessions on a fake kernel of two threads, which the test keeps a handle on, with each
/// cache's secret outside the documents' scratch directory, and sketches solved by a fake.
struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sketches: FakeSketches,
    solvers: Arc<AtomicUsize>,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solvers = Arc::new(AtomicUsize::new(0));
    let (solver, started) = (sketches.clone(), Arc::clone(&solvers));
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    })
    .with_sketch_starter(move || {
        started.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
    });
    Fixture {
        dir,
        kernel,
        sketches,
        solvers,
        sessions,
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    fn open(&self, path: &Path) -> SessionId {
        let response = answer(self.sessions.open(path, false));
        assert!(response.result.is_ok(), "{response:?}");
        response.session.unwrap().id
    }

    /// How many boxes the kernel has been asked to make.
    fn boxes(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count()
    }
}

fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

/// A check that found no problems, of the model rebuilt from revision 1.
fn clean(response: &Response<command::Problems>) {
    assert!(
        response
            .result
            .as_ref()
            .is_ok_and(command::Problems::is_empty),
        "{response:?}"
    );
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision::OPENED
        })
    );
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

#[test]
fn one_sessions_rebuild_never_supersedes_anothers() {
    // MA2 for sessions: A's box is held while B checks its sphere, which would advance a
    // generation every session shared.
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    let b = f.open(&f.file("b.silico", SPHERE));
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let mut held = f.sessions.check(&a);
    assert!(f.kernel.wait_running(1, HANG));
    clean(&answer(f.sessions.check(&b)));
    held = match held.try_take() {
        Ok(response) => panic!("A answered while its box was held: {response:?}"),
        Err(running) => running,
    };
    hold.release();
    clean(&answer(held));
    let model = f.sessions.published(&a).result.unwrap().unwrap();
    assert_eq!(model.revision, Revision::OPENED);
    let fingerprint = model.model.fingerprint(&id("k7f2qx3m")).unwrap();
    assert_eq!(fingerprint.volume, Measure::from_millionths(8_000_000_000));
    assert_eq!(fingerprint.area, Measure::from_millionths(2_800_000_000));
    // The box was made once, never superseded and made again; and A's and B's work came
    // from two clients.
    assert_eq!(f.boxes(), 1);
    let clients: std::collections::BTreeSet<_> =
        f.kernel.calls().iter().map(|c| c.client).collect();
    assert_eq!(clients.len(), 2);
}

#[test]
fn sessions_in_one_directory_build_its_common_steps_once() {
    let f = fixture();
    let a = f.open(&f.file("one/a.silico", BOX));
    clean(&answer(f.sessions.check(&a)));
    assert_eq!(f.boxes(), 1);
    // The same box under another ID, beside it: taken from the directory's cache.
    let b = f.open(&f.file("one/b.silico", SAME_BOX));
    clean(&answer(f.sessions.check(&b)));
    assert_eq!(f.boxes(), 1);
    // In another directory, another cache: made again.
    let c = f.open(&f.file("two/c.silico", SAME_BOX));
    clean(&answer(f.sessions.check(&c)));
    assert_eq!(f.boxes(), 2);
}

#[test]
fn a_second_check_of_a_revision_reads_the_model_its_first_made() {
    let f = fixture();
    let a = f.open(&f.file("a.silico", BOX));
    assert_eq!(f.sessions.published(&a).result.unwrap(), None);
    clean(&answer(f.sessions.check(&a)));
    let first = f.sessions.published(&a).result.unwrap().unwrap();
    let calls = f.kernel.calls().len();
    clean(&answer(f.sessions.check(&a)));
    // No rebuild: the kernel asked nothing, and the model is the same one, not one rebuilt.
    assert_eq!(f.kernel.calls().len(), calls);
    let second = f.sessions.published(&a).result.unwrap().unwrap();
    assert!(Arc::ptr_eq(&first.model, &second.model));
}

#[test]
fn a_check_of_a_document_the_gate_refused_gives_its_problems_and_rebuilds_nothing() {
    let flat = BOX.replace("\"length\": 40", "\"length\": 0");
    let f = fixture();
    let s = f.open(&f.file("flat.silico", &flat));
    let response = answer(f.sessions.check(&s));
    let problems = response.result.unwrap();
    assert_eq!(problems.total(), 1);
    assert_eq!(
        problems.errors()[0].message(),
        "box length must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got 0"
    );
    assert_eq!(response.model, None);
    assert!(f.kernel.calls().is_empty());
    assert_eq!(f.sessions.published(&s).result.unwrap(), None);
}

#[test]
fn a_sessions_sketches_are_solved_by_the_registrys_starter() {
    let sketch = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let f = fixture();
    let s = f.open(&f.file("sketch.silico", sketch));
    clean(&answer(f.sessions.check(&s)));
    // One solver started, which solved the one sketch.
    assert_eq!(f.solvers.load(Ordering::SeqCst), 1);
    assert_eq!(f.sketches.requests().len(), 1);
}

#[test]
fn dropping_the_sessions_mid_rebuild_stops_it_and_returns_at_once() {
    // A step is held, and let go only after the drop returns: a drop that waited for the
    // rebuild before stopping it would never return.
    let f = fixture();
    let path = f.file("a.silico", BOX);
    let a = f.open(&path);
    let _hold = f.kernel.hold(|_| true);
    let check = f.sessions.check(&a);
    assert!(f.kernel.wait_running(1, HANG));
    let Fixture { dir, sessions, .. } = f;
    let lateness = Lateness::start();
    let (dropped, returned) = std::sync::mpsc::channel();
    let at = Instant::now();
    std::thread::spawn(move || {
        drop(sessions);
        let _ = dropped.send(());
    });
    assert!(
        returned.recv_timeout(Duration::from_secs(10)).is_ok(),
        "dropping the sessions waited for the rebuild"
    );
    let took = at.elapsed();
    // A drop stops the rebuild, which takes microseconds: a second for the session's thread
    // to see it and end on a quiet machine, and ten times the machine's lateness on a
    // loaded one.
    let most = allowance(Duration::from_secs(1), lateness.most());
    assert!(
        took < most,
        "dropping the sessions took {took:?}, past {most:?}"
    );
    // The check ends without a model: its session closed under it.
    let response = answer(check);
    assert_eq!(response.model, None);
    let problems = response.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(problems.errors()[0].message(), "no session s1 is open");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BOX);
    drop(dir);
}

#[test]
fn version_gives_the_designs_numbers_and_limits() {
    // As docs/design/m4.1-command-api.md's `version` example and its section 6 give them.
    let version = command::VERSION;
    assert_eq!((version.api, version.format, version.naming), (1, 1, 5));
    let limits = version.limits;
    assert_eq!(limits.document_bytes, 16_777_216);
    assert_eq!(limits.request_bytes, 67_108_864);
    assert_eq!(limits.request_depth, 64);
    assert_eq!(limits.listed, 100);
    assert_eq!(limits.sessions, 16);
    assert_eq!(limits.kernel_seconds, 60);
    assert_eq!(limits.sketch_seconds, 300);
}
