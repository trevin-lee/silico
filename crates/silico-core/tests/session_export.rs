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
use silico_core::command::{CheckModel, Sessions};
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
