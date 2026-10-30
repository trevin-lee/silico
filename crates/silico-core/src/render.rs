//! The render worker's client (docs/design/m4.6-render.md, *In the core*, the render): one
//! worker, started on the first render, drawing one scene at a time in the order they come,
//! on a thread of its own. It's stopped after a large render (by pixels, faces, or
//! triangles), after any render that failed, and when it's been idle, which is what gives
//! its memory back (macOS keeps a large render's after it's dropped); the next render starts
//! another. Each render has its own timeout, from when its scene is sent, so waiting in the
//! queue counts against nothing but the caller's own time. A render queued and cancelled is
//! never sent; one running and cancelled is stopped by killing the worker. Every scene is
//! checked before it's queued, so the worker is sent none it would refuse.
//!
//! The queue isn't bounded: each scene queued is held, meshes and all, until it's drawn or
//! cancelled, so a caller keeps as many queued as it can hold (a session, one at a time).
//!
//! Nothing it returns is a result (ARCHITECTURE.md, the render worker's rule): a PNG, and
//! the ID buffer when asked, for the command that asked.

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use silico_protocol::RenderAdapter;
use silico_protocol::{Generation, Op, PixelId, RenderScene, Reply};

use crate::worker::{Executable, SILICO_BUG, Worker, WorkerKiller, WorkerKind, lock};
use crate::{Error, ErrorCode};

/// The environment variable that names the render worker executable.
pub const RENDER_WORKER_ENV: &str = "SILICO_RENDER_WORKER";

/// The render worker executable: the one [`RENDER_WORKER_ENV`] names, or
/// `silico-render-worker` next to the running executable, as the kernel's and the sketch
/// solver's are found: what a client (`silico serve`) starts its [`RenderClient`] on.
pub fn worker_path() -> Result<std::path::PathBuf, Error> {
    crate::worker::worker_path(
        RENDER_WORKER_ENV,
        "silico-render-worker",
        "render worker",
        "cargo build -p silico-render --bin silico-render-worker",
    )
}

/// How long a render may take from when its scene is sent.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the worker is kept with nothing to draw before it's stopped.
pub const RENDER_IDLE: Duration = Duration::from_secs(120);

/// A render of more pixels than this is large: the worker is stopped once it answers.
pub const LARGE_RENDER_PIXELS: u64 = 1_048_576;

/// A render of more faces than this is large too: each face drawn holds about 75 KiB of
/// the worker's memory, so these hold about as much as [`LARGE_RENDER_PIXELS`] do.
pub const LARGE_RENDER_FACES: usize = 10_000;

/// A render of more triangles than this is large too: each holds about 0.5 KiB of the
/// worker's memory (2 million held 1.2 GiB), so these about 100 MiB.
pub const LARGE_RENDER_TRIANGLES: usize = 200_000;

/// How the client runs its worker. Tests set the times short.
#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub adapter: RenderAdapter,
    /// Each render's own time, from when its scene is sent.
    pub timeout: Duration,
    /// How long the worker is kept idle.
    pub idle: Duration,
    /// Past these, a render is large, and the worker stopped once it answers.
    pub large_pixels: u64,
    pub large_faces: usize,
    pub large_triangles: usize,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            adapter: RenderAdapter::Software,
            timeout: RENDER_TIMEOUT,
            idle: RENDER_IDLE,
            large_pixels: LARGE_RENDER_PIXELS,
            large_faces: LARGE_RENDER_FACES,
            large_triangles: LARGE_RENDER_TRIANGLES,
        }
    }
}

/// A render: the PNG, and the ID buffer if the scene asked for it, one pair a pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub png: Vec<u8>,
    pub ids: Option<Vec<PixelId>>,
}

/// How a render ended: drawn, failed, or cancelled before it answered.
#[derive(Debug)]
pub enum RenderOutcome {
    Done(Result<Rendered, Error>),
    Cancelled,
}

/// A render asked for: its outcome, when it comes, and a way to cancel it. Dropped, the
/// render goes on, its outcome unread (cancel it to stop it).
#[must_use = "a render's outcome is only had by waiting for it"]
pub struct RenderTicket {
    job: Arc<JobState>,
    outcome: Receiver<RenderOutcome>,
}

impl RenderTicket {
    /// Cancels the render: one still queued answers cancelled at once, and is never sent;
    /// one running is stopped by killing the worker (the next render starts another).
    /// Cancelling one that has answered does nothing.
    pub fn cancel(&self) {
        self.job.cancel();
    }

    /// A ticket answered already, by a renderer that answers at once (a test's fake).
    #[cfg(feature = "fake")]
    pub(crate) fn answered(outcome: RenderOutcome) -> Self {
        let (sender, outcome_of) = mpsc::channel();
        let job = Arc::new(JobState {
            stage: Mutex::new(Stage::default()),
            outcome: sender,
        });
        job.answer(outcome);
        RenderTicket {
            job,
            outcome: outcome_of,
        }
    }

    /// A ticket of a render queued and never sent, which only its cancel answers
    /// (`cancelled`, as a render still queued answers one): a test's renderer that holds
    /// what it's sent.
    #[cfg(feature = "fake")]
    pub(crate) fn queued() -> Self {
        let (sender, outcome_of) = mpsc::channel();
        RenderTicket {
            job: Arc::new(JobState {
                stage: Mutex::new(Stage::default()),
                outcome: sender,
            }),
            outcome: outcome_of,
        }
    }

    /// A handle that cancels the render from another thread, while this one waits on it.
    pub fn canceller(&self) -> RenderCancel {
        RenderCancel(Arc::clone(&self.job))
    }

    /// Waits for the render's outcome.
    pub fn wait(self) -> RenderOutcome {
        self.outcome.recv().unwrap_or_else(|_| lost())
    }

    /// Waits up to `timeout` for the outcome, or gives the ticket back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<RenderOutcome, Self> {
        match self.outcome.recv_timeout(timeout) {
            Ok(outcome) => Ok(outcome),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(lost()),
        }
    }

    /// The outcome, if it has come, or the ticket back to wait on.
    pub fn try_take(self) -> Result<RenderOutcome, Self> {
        match self.outcome.try_recv() {
            Ok(outcome) => Ok(outcome),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(lost()),
        }
    }
}

/// Cancels a render, as [`RenderTicket::cancel`] does, from any thread.
#[derive(Clone)]
pub struct RenderCancel(Arc<JobState>);

impl RenderCancel {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// The outcome of a render whose client ended without giving one (a bug).
fn lost() -> RenderOutcome {
    RenderOutcome::Done(Err(Error::new(
        ErrorCode::Internal,
        "the render client stopped before the render answered",
    )
    .with_suggestion(SILICO_BUG)))
}

/// What the client's thread and a ticket share about one render.
struct JobState {
    stage: Mutex<Stage>,
    /// Its outcome's channel, so a cancel can answer a render that hasn't been sent.
    outcome: Sender<RenderOutcome>,
}

/// Where a render is, under one lock: cancelled or not, its scene sent or not, answered or
/// not, and the worker's killer while it runs on it.
#[derive(Default)]
struct Stage {
    cancelled: bool,
    sent: bool,
    answered: bool,
    killer: Option<WorkerKiller>,
}

impl JobState {
    /// Cancels it: a render not yet sent answers cancelled here, and its turn is skipped;
    /// one running has its worker killed, and its turn answers cancelled when the call ends.
    fn cancel(&self) {
        let mut stage = lock(&self.stage);
        if stage.cancelled {
            return;
        }
        stage.cancelled = true;
        match (&stage.killer, stage.sent) {
            (Some(killer), _) => {
                killer.kill();
            }
            (None, false) => self.answer_under(&mut stage, RenderOutcome::Cancelled),
            // Sent and answered, or about to be: its turn sees the cancel.
            (None, true) => {}
        }
    }

    /// Answers it, once: a render's first answer is its outcome.
    fn answer(&self, outcome: RenderOutcome) {
        self.answer_under(&mut lock(&self.stage), outcome);
    }

    fn answer_under(&self, stage: &mut Stage, outcome: RenderOutcome) {
        if !stage.answered {
            stage.answered = true;
            let _ = self.outcome.send(outcome);
        }
    }
}

/// A render queued. However it leaves the queue, it's answered: dropped unanswered (the
/// client's thread ending without drawing it, a bug), it answers that.
struct Job {
    /// Taken for its turn.
    scene: Option<RenderScene>,
    state: Arc<JobState>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.state.answer(lost());
    }
}

/// The render worker's client: renders in turn, on a worker of its own.
pub struct RenderClient {
    jobs: Mutex<Option<Sender<Job>>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

/// What the client and its thread share. Its locks and a render's `stage` are never held
/// together, so no two threads can each hold one the other waits for: a render's `stage`
/// is held only with its worker's own lock, taken under it when a cancel kills the worker.
#[derive(Default)]
struct Shared {
    /// The worker's process ID while it runs, for tests that watch it stop.
    pid: Mutex<Option<u32>>,
    /// The render under way, so a client closing can stop it.
    current: Mutex<Option<Arc<JobState>>>,
    /// Set as the client closes: the renders still queued answer cancelled, unsent.
    closing: AtomicBool,
}

impl RenderClient {
    /// The client of the render worker at `exe`, checked to be a build (it isn't started
    /// until the first render).
    pub fn start(exe: &Path, settings: RenderSettings) -> Result<Self, Error> {
        let args: Vec<OsString> = settings.adapter.args().iter().map(OsString::from).collect();
        let exe = Executable::with_args(exe, WorkerKind::Render, args)?;
        let (jobs, queue) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("render".to_owned())
            .spawn(move || serve(&exe, &settings, &queue, &theirs))
            .map_err(|e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("couldn't start the render client's thread: {e}"),
                )
            })?;
        Ok(RenderClient {
            jobs: Mutex::new(Some(jobs)),
            thread: Some(thread),
            shared,
        })
    }

    /// Queues `scene`. It's checked first ([`RenderScene::check`]): the command builds
    /// scenes from values it has checked, so one that fails is Silico's bug, answered
    /// `internal` at once and never sent.
    pub fn render(&self, scene: RenderScene) -> RenderTicket {
        let (outcome, reply) = mpsc::channel();
        let state = Arc::new(JobState {
            stage: Mutex::new(Stage::default()),
            outcome,
        });
        let ticket = RenderTicket {
            job: Arc::clone(&state),
            outcome: reply,
        };
        if let Err(invalid) = scene.check() {
            state.answer(RenderOutcome::Done(Err(Error::new(
                ErrorCode::Internal,
                "the core built a scene the render worker can't draw",
            )
            .with_parameter(invalid.parameter)
            .with_cause(invalid.message)
            .with_suggestion(SILICO_BUG))));
            return ticket;
        }
        let job = Job {
            scene: Some(scene),
            state,
        };
        if let Some(jobs) = lock(&self.jobs).as_ref() {
            // A send fails only once the thread is gone: the job comes back, and is
            // answered as it's dropped.
            let _ = jobs.send(job);
        }
        ticket
    }

    /// The worker's process ID, while one runs.
    pub fn pid(&self) -> Option<u32> {
        *lock(&self.shared.pid)
    }
}

impl Drop for RenderClient {
    /// Closes the client as a session closing ends its renders: the one running stopped (its
    /// worker killed), those queued answered cancelled, unsent; then the worker is stopped.
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        lock(&self.jobs).take();
        // Taken out of its lock before it's cancelled: no lock is held while another is
        // taken (see `Shared`).
        let current = lock(&self.shared.current).clone();
        if let Some(current) = current {
            current.cancel();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The client's thread: each render in turn, the worker started for it as needed and
/// stopped when it should be.
fn serve(exe: &Arc<Executable>, settings: &RenderSettings, queue: &Receiver<Job>, shared: &Shared) {
    let mut server = Server {
        exe,
        settings,
        shared,
        worker: None,
        drew: Instant::now(),
    };
    loop {
        if shared.closing.load(Ordering::SeqCst) {
            while let Ok(job) = queue.try_recv() {
                job.state.cancel();
            }
            break;
        }
        let mut job = match server.worker {
            // Idle from when it last drew, whatever else came.
            Some(_) => {
                match queue.recv_timeout(settings.idle.saturating_sub(server.drew.elapsed())) {
                    Ok(job) => job,
                    Err(RecvTimeoutError::Timeout) => {
                        server.stop();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match queue.recv() {
                Ok(job) => job,
                Err(_) => break,
            },
        };
        // Made current before the client's closing is looked at again: a client closing
        // sets that, then cancels what's current, so between them this render is seen.
        *lock(&shared.current) = Some(Arc::clone(&job.state));
        if shared.closing.load(Ordering::SeqCst) {
            job.state.cancel();
            lock(&shared.current).take();
            continue;
        }
        // A render cancelled while queued has answered: its turn is skipped.
        let skipped = {
            let mut stage = lock(&job.state.stage);
            if !stage.cancelled {
                stage.sent = true;
            }
            stage.cancelled
        };
        let scene = job.scene.take();
        let Some(scene) = scene.filter(|_| !skipped) else {
            lock(&shared.current).take();
            continue;
        };
        let outcome = server.turn(scene, &job.state);
        lock(&shared.current).take();
        job.state.answer(outcome);
    }
    server.stop();
}

/// The client's thread's state: its worker, while one runs, and when it last drew.
struct Server<'a> {
    exe: &'a Arc<Executable>,
    settings: &'a RenderSettings,
    shared: &'a Shared,
    worker: Option<Worker>,
    drew: Instant,
}

impl Server<'_> {
    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.shut_down();
        }
        *lock(&self.shared.pid) = None;
    }

    /// One render's turn: the worker started if there's none, the scene sent, and the
    /// worker stopped after if it should be.
    fn turn(&mut self, scene: RenderScene, state: &JobState) -> RenderOutcome {
        if self
            .worker
            .as_mut()
            .is_some_and(|w| w.is_stopped() || w.has_exited())
        {
            self.stop();
        }
        let running = match &mut self.worker {
            Some(w) => w,
            None => match self.exe.spawn() {
                Ok(w) => {
                    *lock(&self.shared.pid) = Some(w.pid());
                    self.drew = Instant::now();
                    self.worker.insert(w)
                }
                Err(e) => return RenderOutcome::Done(Err(e)),
            },
        };
        {
            // The ticket can kill the worker from here on; a cancel that came while it
            // started is seen here, and the scene isn't sent.
            let mut stage = lock(&state.stage);
            if stage.cancelled {
                return RenderOutcome::Cancelled;
            }
            stage.killer = Some(running.killer());
        }
        let settings = self.settings;
        let faces: usize = scene
            .bodies
            .iter()
            .map(|b| b.mesh.face_triangles.len())
            .sum();
        let triangles: usize = scene.bodies.iter().map(|b| b.mesh.triangles.len()).sum();
        let large = u64::from(scene.width) * u64::from(scene.height) > settings.large_pixels
            || faces > settings.large_faces
            || triangles > settings.large_triangles;
        let (width, height, ids) = (scene.width, scene.height, scene.ids);
        // The render worker has no generations: nothing it draws is superseded, only
        // cancelled, by killing it. Every render is the first generation's.
        let reply = running.call(Generation(1), Op::Render(Box::new(scene)), settings.timeout);
        let cancelled = {
            let mut stage = lock(&state.stage);
            stage.killer = None;
            stage.cancelled
        };
        if cancelled {
            // Killed, or about to be: no worker that was is trusted.
            self.stop();
            return RenderOutcome::Cancelled;
        }
        self.drew = Instant::now();
        let result = match reply {
            Ok(Reply::Rendered { png, ids: got }) => rendered(width, height, ids, png, got),
            Ok(other) => Err(Error::new(
                ErrorCode::ProtocolError,
                "the render worker answered Render with the wrong kind of reply",
            )
            .with_cause(format!("it answered {}", other.kind()))
            .with_suggestion("render again: the next render starts another worker")),
            Err(e) => Err(e),
        };
        // Any failure, and a large render, stop it (its memory isn't given back while it
        // runs, and nothing vouches for its context after a failure). The scene was
        // checked, so a worker refusing it is at fault.
        let result = result.map_err(|e| match e.code() {
            ErrorCode::InvalidInput => Error::new(
                ErrorCode::ProtocolError,
                "the render worker refused a scene the core checked",
            )
            .with_cause(e.to_string())
            .with_suggestion("render again: the next render starts another worker"),
            _ => e,
        });
        if result.is_err() || large {
            self.stop();
        }
        RenderOutcome::Done(result)
    }
}

/// The worker's render, if it's what was asked: a PNG, and the ID buffer exactly when asked,
/// a pair for each of the scene's pixels.
fn rendered(
    width: u32,
    height: u32,
    asked: bool,
    png: Vec<u8>,
    ids: Option<Vec<PixelId>>,
) -> Result<Rendered, Error> {
    let pixels = u64::from(width) * u64::from(height);
    let fault = match &ids {
        _ if png.is_empty() => Some("no image".to_owned()),
        Some(_) if !asked => Some("an ID buffer that wasn't asked for".to_owned()),
        None if asked => Some("no ID buffer, though one was asked for".to_owned()),
        Some(ids) if ids.len() as u64 != pixels => {
            Some(format!("{} IDs for {pixels} pixels", ids.len()))
        }
        _ => None,
    };
    match fault {
        None => Ok(Rendered { png, ids }),
        Some(fault) => Err(Error::new(
            ErrorCode::ProtocolError,
            "the render worker answered with a render other than asked",
        )
        .with_cause(fault)
        .with_suggestion("render again: the next render starts another worker")),
    }
}
