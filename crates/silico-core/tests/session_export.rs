//! `Sessions::export` (M4.1's `export`): the file rules and the settle a session gives the
//! export (decisions 34 and 35), on the fake kernel, each solid's export mesh the
//! tetrahedron built by hand (export_report.rs). The command's own steps are
//! export_command.rs's.

use std::sync::Arc;
use std::time::Duration;

use super::example::EXAMPLE;
use super::export_report::tetra;
use super::listing::{answer, fixture};
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::export::{Export, ExportFormat};
use silico_core::command::logs::LogLevel;
use silico_core::command::{CheckModel, SessionId, Sessions};
use silico_core::document::Length;
use silico_core::fake::{FakeClient, FakeKernel, FakeSketches};
use silico_core::kernel::{Clients, Kernel, KernelRequest, Op};
use silico_core::pool::Pending;
use silico_core::sketch::SketchSolver;
use silico_protocol::Generation;
use silico_protocol::Reply;

fn request(path: std::path::PathBuf) -> Export {
    Export {
        format: ExportFormat::ThreeMf {
            tolerance: Length::rounded(0.1).unwrap(),
        },
        path,
        replace: false,
        features: None,
    }
}

#[test]
fn an_export_to_a_document_a_session_holds_is_refused_and_the_document_kept() {
    // Decision 35: a file a command writes is never a document the sessions hold, whatever
    // it's named. One named `.3mf`, open in a session, isn't replaced by its own export,
    // replace or not; nothing is meshed.
    let f = fixture();
    f.kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
        _ => None,
    });
    let s = f.open("held.3mf", EXAMPLE);
    let held = f.dir.path().join("held.3mf");
    let mut asked = request(held.clone());
    asked.replace = true;
    let problems = answer(f.sessions.export(&s, asked)).result.unwrap_err();
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("path")),
        "{e:?}"
    );
    assert!(
        e.message().contains("is a document a session holds"),
        "{e:?}"
    );
    assert_eq!(std::fs::read_to_string(&held).unwrap(), EXAMPLE);
    assert!(
        !f.kernel
            .calls()
            .iter()
            .any(|c| matches!(c.op, Op::ExportMesh { .. }))
    );
}

/// A fake kernel whose clients answer what they began though a read is interrupted (as a
/// worker whose answer was on its way when the cancel came): every advance but the
/// interrupt's, to the last generation, is the fake's.
struct AnswersAnyway(Arc<FakeKernel>);

struct AnswersAnywayClient(FakeClient);

impl Clients for AnswersAnyway {
    type Client = AnswersAnywayClient;

    fn client(&self) -> AnswersAnywayClient {
        AnswersAnywayClient(self.0.client())
    }
}

impl Kernel for AnswersAnywayClient {
    fn identity(&self) -> &str {
        self.0.identity()
    }

    fn generation(&self) -> Generation {
        self.0.generation()
    }

    fn advance(&self, generation: Generation) {
        if generation != Generation(u64::MAX) {
            self.0.advance(generation);
        }
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.0.submit(generation, request)
    }
}

#[test]
fn an_export_cancelled_once_meshed_answers_cancelled_and_writes_no_file() {
    // Decision 34: a cancelled command leaves no trace. The mesh is held in the kernel, the
    // export cancelled, then the mesh let go, answered though the cancel came first: the
    // export, its mesh in hand, settles before it writes, the settle refuses, and nothing
    // is written, neither the file nor one beside it.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
        _ => None,
    });
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        AnswersAnyway(Arc::clone(&kernel)),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let file = dir.path().join("example.silico");
    std::fs::write(&file, EXAMPLE).unwrap();
    let s = answer(sessions.open(file, false)).session.unwrap().id;
    // Rebuilt first, so the one request running once it's asked is the export's mesh.
    answer(sessions.check(&s, CheckModel::Current { fresh: false }))
        .result
        .unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let hold = kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let running = sessions.export(&s, request(out.join("part.3mf")));
    assert!(kernel.wait_running(1, Duration::from_secs(60)));
    assert!(running.stopper().cancel());
    hold.release();
    let refused = answer(running).result.unwrap_err();
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::Cancelled,
        "{refused:?}"
    );
    // The read's own client is dropped once the export returns, retiring it.
    assert!(kernel.wait_idle(Duration::from_secs(60)));
    sessions.close(&s).wait();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    // Meshed, and answered: the settle, not the kernel, kept the file from being written.
    assert_eq!(
        kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::ExportMesh { .. }))
            .count(),
        1
    );
}

#[test]
fn a_step_export_cancelled_while_it_writes_stops_and_answers_cancelled_writing_nothing() {
    // Decision 34: the STEP worker's writing is stopped with the read (its worker killed;
    // the fake's request superseded). Held in the fake, the export is cancelled: it answers
    // `cancelled` at once, not when the writing would have ended, and leaves no file.
    let f = fixture();
    let sessions = f.sessions.exporting_step(Arc::clone(&f.kernel));
    let file = f.dir.path().join("example.silico");
    std::fs::write(&file, EXAMPLE).unwrap();
    let s = answer(sessions.open(file, false)).session.unwrap().id;
    answer(sessions.check(&s, CheckModel::Current { fresh: false }))
        .result
        .unwrap();
    let out = f.dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let hold = f.kernel.hold(|op| matches!(op, Op::ExportStep(_)));
    let running = sessions.export(
        &s,
        Export {
            format: ExportFormat::Step,
            path: out.join("part.step"),
            replace: false,
            features: None,
        },
    );
    assert!(f.kernel.wait_running(1, Duration::from_secs(60)));
    assert!(running.stopper().cancel());
    let refused = match running.wait_timeout(Duration::from_secs(10)) {
        Ok(response) => response.result.unwrap_err(),
        Err(_) => panic!("the export went on writing past its cancel"),
    };
    drop(hold);
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::Cancelled,
        "{refused:?}"
    );
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
}

/// The export meshes submitted to `kernel` so far, whether or not they started.
fn asked_meshes(kernel: &FakeKernel) -> usize {
    kernel
        .asked()
        .iter()
        .filter(|c| matches!(c.op, Op::ExportMesh { .. }))
        .count()
}

/// A fixture whose export meshes are the tetrahedron, with `names` open in sessions, each
/// the example, rebuilt, so the one kernel work after is the exports'.
fn exporting(
    names: &[&str],
) -> (
    super::listing::Fixture,
    Vec<silico_core::command::SessionId>,
) {
    let f = fixture();
    f.kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
        _ => None,
    });
    let sessions = names
        .iter()
        .map(|name| {
            let s = f.open(name, EXAMPLE);
            answer(f.sessions.check(&s, CheckModel::Current { fresh: false }))
                .result
                .unwrap();
            s
        })
        .collect();
    (f, sessions)
}

#[test]
fn two_sessions_exporting_at_once_mesh_one_at_a_time() {
    // One export mesh in flight across every session, so sessions exporting at once can't
    // mesh on every core at once, each mesh at a fine tolerance gigabytes. Two sessions'
    // exports, a kernel that runs two requests at once, the first mesh held: the other's
    // isn't asked until it's done, then both are written.
    let (f, s) = exporting(&["a.silico", "b.silico"]);
    let hold = f.kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let first = f
        .sessions
        .export(&s[0], request(f.dir.path().join("a.3mf")));
    let second = f
        .sessions
        .export(&s[1], request(f.dir.path().join("b.3mf")));
    assert!(f.kernel.wait_running(1, Duration::from_secs(60)));
    // Long enough for the other session's mesh to be asked and start, were it let.
    assert!(!f.kernel.wait_running(2, Duration::from_millis(500)));
    assert_eq!(asked_meshes(&f.kernel), 1);
    hold.release();
    answer(first).result.unwrap();
    answer(second).result.unwrap();
    assert_eq!(asked_meshes(&f.kernel), 2);
    assert!(f.dir.path().join("a.3mf").is_file());
    assert!(f.dir.path().join("b.3mf").is_file());
}

#[test]
fn an_export_waiting_its_turn_to_mesh_stops_at_once_when_cancelled() {
    // Decision 34: a wait for another session's mesh is cancelled as a wait for the kernel
    // is. The first session's mesh held; the second's export, cancelled, answers at once,
    // never asking for a mesh, writing nothing; then the first is let go and written.
    let (f, s) = exporting(&["a.silico", "b.silico"]);
    let hold = f.kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let first = f
        .sessions
        .export(&s[0], request(f.dir.path().join("a.3mf")));
    assert!(f.kernel.wait_running(1, Duration::from_secs(60)));
    let second = f
        .sessions
        .export(&s[1], request(f.dir.path().join("b.3mf")));
    // Waiting its turn, or about to: a cancel stops it wherever it is.
    assert!(!f.kernel.wait_running(2, Duration::from_millis(200)));
    assert!(second.stopper().cancel());
    let refused = match second.wait_timeout(Duration::from_secs(10)) {
        Ok(response) => response.result.unwrap_err(),
        Err(_) => panic!("the export went on waiting its turn past its cancel"),
    };
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::Cancelled,
        "{refused:?}"
    );
    assert_eq!(asked_meshes(&f.kernel), 1);
    hold.release();
    answer(first).result.unwrap();
    assert!(f.dir.path().join("a.3mf").is_file());
    assert!(!f.dir.path().join("b.3mf").exists());
    // The turn given back by the cancelled wait: the second session exports once asked again.
    answer(
        f.sessions
            .export(&s[1], request(f.dir.path().join("b.3mf"))),
    )
    .result
    .unwrap();
    assert!(f.dir.path().join("b.3mf").is_file());
}

/// Waits until `session`'s logs say its export waits for another export's mesh, so what a
/// test does next finds it waiting (the process's subscriber keeping session logs).
fn waiting_its_turn<K: Clients + 'static>(sessions: &Sessions<K>, session: &SessionId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let query = crate::logged::query(LogLevel::Info, None, None, 1000);
        let logs = sessions.read_logs(session, &query).result.unwrap();
        if logs
            .events
            .iter()
            .any(|e| e.message.starts_with("waiting for another export's mesh"))
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the export never waited its turn: {:?}",
            logs.events
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_export_waiting_its_turn_to_mesh_when_its_session_closes_answers_closed_not_cancelled() {
    // An export reads the current model, on its session's thread, so `close`, queued behind
    // it, never stops it: its session closes under it as the sessions are dropped (every
    // session closed, each command running stopped as closed). The first session's mesh
    // held, answered though its session closes (`AnswersAnyway`), so it keeps the turn; the
    // second's export, waiting its turn, answers as of a session that closed (decision 34;
    // as a published render does), never `cancelled`, since no client stopped it, and
    // writes nothing, never asking for a mesh.
    crate::logged::keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
        _ => None,
    });
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        AnswersAnyway(Arc::clone(&kernel)),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let s: Vec<_> = ["a.silico", "b.silico"]
        .iter()
        .map(|name| {
            let file = dir.path().join(name);
            std::fs::write(&file, EXAMPLE).unwrap();
            let s = answer(sessions.open(file, false)).session.unwrap().id;
            // Rebuilt first, so the one kernel work after is the exports'.
            answer(sessions.check(&s, CheckModel::Current { fresh: false }))
                .result
                .unwrap();
            s
        })
        .collect();
    let hold = kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let first = sessions.export(&s[0], request(dir.path().join("a.3mf")));
    // The first session's mesh asked, and held: its export holds the turn.
    assert!(kernel.wait_running(1, Duration::from_secs(60)));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while asked_meshes(&kernel) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(asked_meshes(&kernel), 1);
    let second = sessions.export(&s[1], request(dir.path().join("b.3mf")));
    // The second export waiting its turn, as its session's logs say, before the close.
    waiting_its_turn(&sessions, &s[1]);
    // Every session closed; the drop waits for the first's mesh, held, so it's let go on a
    // thread of its own.
    let dropping = std::thread::spawn(move || drop(sessions));
    let refused = answer(second).result.unwrap_err();
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("session"), None),
        "{e:?}"
    );
    assert!(!dir.path().join("b.3mf").exists());
    // Only the first session's mesh was asked of the kernel, still held.
    assert_eq!(asked_meshes(&kernel), 1);
    hold.release();
    answer(first);
    dropping.join().unwrap();
    assert_eq!(asked_meshes(&kernel), 1);
    assert!(!dir.path().join("b.3mf").exists());
}
