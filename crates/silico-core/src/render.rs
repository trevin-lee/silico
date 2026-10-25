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
use std::time::Duration;

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
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            adapter: RenderAdapter::Software,
            timeout: RENDER_TIMEOUT,
            idle: RENDER_IDLE,
            large_pixels: LARGE_RENDER_PIXELS,
            large_faces: LARGE_RENDER_FACES,
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
    /// Cancels the render: one still queued is never sent; one running is stopped by
    /// killing the worker (the next render starts another). Cancelling one that has
    /// answered does nothing.
    pub fn cancel(&self) {
        self.job.cancelled.store(true, Ordering::SeqCst);
        if let Some(killer) = lock(&self.job.killer).as_ref() {
            killer.kill();
        }
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
#[derive(Default)]
struct JobState {
    cancelled: AtomicBool,
    /// The worker's killer while the render runs on it.
    killer: Mutex<Option<WorkerKiller>>,
}

struct Job {
    scene: RenderScene,
    state: Arc<JobState>,
    outcome: Sender<RenderOutcome>,
}

/// The render worker's client: renders in turn, on a worker of its own.
pub struct RenderClient {
    jobs: Mutex<Option<Sender<Job>>>,
    thread: Option<JoinHandle<()>>,
    /// The worker's process ID while it runs, for tests that watch it stop.
    pid: Arc<Mutex<Option<u32>>>,
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
        let pid = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&pid);
        let thread = std::thread::Builder::new()
            .name("render".to_owned())
            .spawn(move || serve(&exe, &settings, &queue, &seen))
            .map_err(|e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("couldn't start the render client's thread: {e}"),
                )
            })?;
        Ok(RenderClient {
            jobs: Mutex::new(Some(jobs)),
            thread: Some(thread),
            pid,
        })
    }

    /// Queues `scene`, which must have passed [`RenderScene::check`] (the worker checks it
    /// again, and refuses one that doesn't).
    pub fn render(&self, scene: RenderScene) -> RenderTicket {
        let state = Arc::new(JobState::default());
        let (outcome, reply) = mpsc::channel();
        let job = Job {
            scene,
            state: Arc::clone(&state),
            outcome,
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
        *lock(&self.pid)
    }
}

impl Drop for RenderClient {
    fn drop(&mut self) {
        // Closing the queue ends the thread, which stops the worker.
        lock(&self.jobs).take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The client's thread: each render in turn, the worker started for it as needed and
/// stopped when it should be.
fn serve(
    exe: &Arc<Executable>,
    settings: &RenderSettings,
    queue: &Receiver<Job>,
    pid: &Mutex<Option<u32>>,
) {
    let mut worker: Option<Worker> = None;
    let stop = |worker: &mut Option<Worker>| {
        if let Some(mut w) = worker.take() {
            w.shut_down();
        }
        *lock(pid) = None;
    };
    loop {
        let job = match worker {
            Some(_) => match queue.recv_timeout(settings.idle) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => {
                    stop(&mut worker);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match queue.recv() {
                Ok(job) => job,
                Err(_) => break,
            },
        };
        if job.state.cancelled.load(Ordering::SeqCst) {
            let _ = job.outcome.send(RenderOutcome::Cancelled);
            continue;
        }
        if worker
            .as_mut()
            .is_some_and(|w| w.is_stopped() || w.has_exited())
        {
            stop(&mut worker);
        }
        let running = match &mut worker {
            Some(w) => w,
            None => match exe.spawn() {
                Ok(w) => {
                    *lock(pid) = Some(w.pid());
                    worker.insert(w)
                }
                Err(e) => {
                    let _ = job.outcome.send(RenderOutcome::Done(Err(e)));
                    continue;
                }
            },
        };
        // The ticket can kill the worker from here on; a cancel that came just before is
        // seen next, and the scene isn't sent.
        *lock(&job.state.killer) = Some(running.killer());
        if job.state.cancelled.load(Ordering::SeqCst) {
            lock(&job.state.killer).take();
            let _ = job.outcome.send(RenderOutcome::Cancelled);
            continue;
        }
        let large = u64::from(job.scene.width) * u64::from(job.scene.height)
            > settings.large_pixels
            || job
                .scene
                .bodies
                .iter()
                .map(|b| b.mesh.face_triangles.len())
                .sum::<usize>()
                > settings.large_faces;
        let (width, height, ids) = (job.scene.width, job.scene.height, job.scene.ids);
        let reply = running.call(
            Generation(1),
            Op::Render(Box::new(job.scene)),
            settings.timeout,
        );
        lock(&job.state.killer).take();
        if job.state.cancelled.load(Ordering::SeqCst) {
            // Killed, or about to be: no worker that was is trusted.
            stop(&mut worker);
            let _ = job.outcome.send(RenderOutcome::Cancelled);
            continue;
        }
        let result = match reply {
            Ok(Reply::Rendered { png, ids: got }) => rendered(width, height, ids, png, got),
            Ok(other) => Err(Error::new(
                ErrorCode::ProtocolError,
                "the render worker answered Render with the wrong kind of reply",
            )
            .with_cause(format!("a {} reply", other.kind()))),
            Err(e) => Err(e),
        };
        // A scene the worker refused as invalid left it as it was; any other failure, and
        // a large render, stop it (its memory isn't given back while it runs).
        match &result {
            Err(e) if e.code() == ErrorCode::InvalidInput => {}
            Err(_) => stop(&mut worker),
            Ok(_) if large => stop(&mut worker),
            Ok(_) => {}
        }
        let _ = job.outcome.send(RenderOutcome::Done(result));
    }
    stop(&mut worker);
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
