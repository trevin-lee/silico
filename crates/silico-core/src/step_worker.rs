//! The STEP worker (docs/ports/step-export.md): OCCT's STEP writer in a process of its own
//! (silico-kernel's `silico-step-worker`), so the kernel worker carries none of STEP's
//! toolkits. `export` writes STEP through a [`StepExporter`]: the STEP worker, started for each
//! export ([`StepWorkers`]; exports are rare, and a fresh process per file is deterministic by
//! construction), or a test's stand-in (the fake kernel, with the `fake` feature).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use silico_protocol::{ExportStep, Generation, StepFile};

use crate::Error;
use crate::kernel::KernelRequest;
use crate::worker::{Executable, Worker, WorkerKind, lock};

/// The environment variable naming the STEP worker executable, for when it isn't next to the
/// running one.
pub const STEP_WORKER_ENV: &str = "SILICO_STEP_WORKER";

/// What writes `export`'s STEP files.
pub trait StepExporter: Send + Sync {
    /// `export` written as one STEP file, and the identity of what wrote it (the STEP
    /// worker's build, as a kernel's identity is its worker's: an export's bytes are its
    /// writer's), checked as [`KernelRequest::export_step`] checks a reply. Once the writing
    /// has started, `started` is given what stops it (the worker killed), which a cancel or
    /// the session's close calls (decision 34): stopped, it answers an error, never a file.
    fn export_step(
        &self,
        export: ExportStep,
        started: &dyn Fn(StepStop),
    ) -> Result<(StepFile, String), Error>;
}

/// A shared exporter writes as it does.
impl<T: StepExporter + ?Sized> StepExporter for Arc<T> {
    fn export_step(
        &self,
        export: ExportStep,
        started: &dyn Fn(StepStop),
    ) -> Result<(StepFile, String), Error> {
        (**self).export_step(export, started)
    }
}

/// What stops a STEP export under way ([`StepExporter::export_step`]'s `started`).
pub struct StepStop(Box<dyn Fn() + Send + Sync>);

impl StepStop {
    pub fn new(stop: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Box::new(stop))
    }

    /// Stops the export.
    pub fn stop(&self) {
        (self.0)();
    }
}

/// The STEP worker, found once (the one [`STEP_WORKER_ENV`] names, or `silico-step-worker`
/// beside the running executable) and started for each export, then stopped. Not found, or
/// not startable, an export is `worker_unavailable`, saying how to build or name it.
#[derive(Default)]
pub struct StepWorkers {
    /// The executable, once found: its identity, and its stamp, so a build replaced under a
    /// running Silico is refused, not taken as this one.
    found: Mutex<Option<Arc<Executable>>>,
}

impl StepWorkers {
    pub fn new() -> Self {
        Self::default()
    }

    fn executable(&self) -> Result<Arc<Executable>, Error> {
        let mut found = lock(&self.found);
        if let Some(exe) = found.as_ref() {
            return Ok(Arc::clone(exe));
        }
        let exe = Executable::of(&worker_path()?, WorkerKind::Step)?;
        *found = Some(Arc::clone(&exe));
        Ok(exe)
    }
}

impl StepExporter for StepWorkers {
    fn export_step(
        &self,
        export: ExportStep,
        started: &dyn Fn(StepStop),
    ) -> Result<(StepFile, String), Error> {
        let request = KernelRequest::export_step(export)?;
        let exe = self.executable()?;
        let mut worker = exe.spawn()?;
        let killer = worker.killer();
        started(StepStop::new(move || {
            killer.kill();
        }));
        let file = request.run_on(&mut worker, Generation(0))?;
        Ok((file, exe.identity().to_owned()))
    }
}

/// One STEP worker, for tests and tools: started at once and kept for its requests.
pub struct StepWorker {
    worker: Worker,
}

impl StepWorker {
    /// The STEP worker [`STEP_WORKER_ENV`] names, or `silico-step-worker` beside the running
    /// executable (or one directory up, where a test binary's worker is).
    pub fn start() -> Result<Self, Error> {
        Self::start_worker(&worker_path()?)
    }

    /// The STEP worker at `exe`.
    pub fn start_worker(exe: &Path) -> Result<Self, Error> {
        Ok(Self {
            worker: Worker::spawn_as(exe, WorkerKind::Step)?,
        })
    }

    /// [`KernelRequest::export_step`], run on this worker.
    pub fn export_step(&mut self, export: ExportStep) -> Result<StepFile, Error> {
        KernelRequest::export_step(export)?.run_on(&mut self.worker, Generation(0))
    }
}

/// The STEP worker executable: [`STEP_WORKER_ENV`], or `silico-step-worker` next to the
/// running executable.
fn worker_path() -> Result<PathBuf, Error> {
    crate::worker::worker_path(
        STEP_WORKER_ENV,
        "silico-step-worker",
        "STEP worker",
        "cargo build -p silico-kernel --bin silico-step-worker",
    )
}
