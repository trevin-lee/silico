//! The render worker's client (docs/design/m4.6-render.md, *In the core*, the render): one
//! worker, started on the first render, drawing one scene at a time in the order they come,
//! on a thread of its own. It's stopped after a large render (by pixels or by faces), after
//! any render that failed, and when it's been idle, which is what gives its memory back
//! (macOS keeps a large render's after it's dropped); the next render starts another. Each
//! render has its own timeout, from when its scene is sent, so waiting in the queue counts
//! against nothing but the caller's own time. A render queued and cancelled is never sent;
//! one running and cancelled is stopped by killing the worker.
//!
//! Nothing it returns is a result (ARCHITECTURE.md, the render worker's rule): a PNG, and
//! the ID buffer when asked, for the command that asked.

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use silico_protocol::{Generation, Op, PixelId, RenderScene, Reply};

use crate::worker::{Executable, Worker, WorkerKiller, WorkerKind};
use crate::{Error, ErrorCode};

/// The environment variable that names the render worker executable.
pub const RENDER_WORKER_ENV: &str = "SILICO_RENDER_WORKER";

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

/// Which adapter the worker may draw on: the core's one say about the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderAdapter {
    /// Only a CPU adapter (Mesa's lavapipe), or the render is `worker_unavailable`.
    Software,
    /// A CPU adapter if there is one, then a discrete GPU, then any.
    Any,
}

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

/// A render asked for: its outcome, when it comes, and a way to cancel it.
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

    /// Waits for the render's outcome.
    pub fn wait(self) -> RenderOutcome {
        self.outcome.recv().unwrap_or_else(|_| {
            RenderOutcome::Done(Err(Error::new(
                ErrorCode::Internal,
                "the render client stopped before the render answered",
            )))
        })
    }
}

/// What the client's thread and a ticket share about one render.
struct JobState {
    stage: Mutex<Stage>,
    /// Its outcome's channel, so a cancel can answer a render that hasn't been sent.
    outcome: Sender<RenderOutcome>,
}

/// Where a render is, under one lock: cancelled or not, its scene sent or not, and the
/// worker's killer while it runs on it.
#[derive(Default)]
struct Stage {
    cancelled: bool,
    sent: bool,
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
            (None, false) => {
                let _ = self.outcome.send(RenderOutcome::Cancelled);
            }
            // Sent and answered, or about to be: its turn sees the cancel.
            (None, true) => {}
        }
    }

    fn answer(&self, outcome: RenderOutcome) {
        let _ = self.outcome.send(outcome);
    }
}

struct Job {
    scene: RenderScene,
    state: Arc<JobState>,
}

/// The render worker's client: renders in turn, on a worker of its own.
pub struct RenderClient {
    jobs: Mutex<Option<Sender<Job>>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

/// What the client and its thread share.
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
        let args: Vec<OsString> = match settings.adapter {
            RenderAdapter::Software => vec![],
            RenderAdapter::Any => vec!["--any-adapter".into()],
        };
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

    /// Queues `scene`, which must have passed [`RenderScene::check`] (the worker checks it
    /// again, and refuses one that doesn't).
    pub fn render(&self, scene: RenderScene) -> RenderTicket {
        let (outcome, reply) = mpsc::channel();
        let state = Arc::new(JobState {
            stage: Mutex::new(Stage::default()),
            outcome,
        });
        let job = Job {
            scene,
            state: Arc::clone(&state),
        };
        if let Some(jobs) = lock(&self.jobs).as_ref() {
            // A send fails only once the thread is gone; the ticket then says so.
            let _ = jobs.send(job);
        }
        RenderTicket {
            job: state,
            outcome: reply,
        }
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
        if let Some(current) = lock(&self.shared.current).as_ref() {
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
        let job = match server.worker {
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
        {
            // A render cancelled while queued has answered: its turn is skipped.
            let mut stage = lock(&job.state.stage);
            if stage.cancelled {
                continue;
            }
            stage.sent = true;
        }
        *lock(&shared.current) = Some(Arc::clone(&job.state));
        let outcome = server.turn(job.scene, &job.state);
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
            .with_cause(format!("a {} reply", other.kind()))),
            Err(e) => Err(e),
        };
        // A scene the worker refused as invalid left it as it was; any other failure, and a
        // large render, stop it (its memory isn't given back while it runs).
        match &result {
            Err(e) if e.code() == ErrorCode::InvalidInput => {}
            Err(_) => self.stop(),
            Ok(_) if large => self.stop(),
            Ok(_) => {}
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
