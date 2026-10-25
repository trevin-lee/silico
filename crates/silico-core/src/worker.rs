//! The core's side of a worker, the kernel's or the sketch solver's (ARCHITECTURE.md,
//! Process model, and [`WorkerKind`], which names it in errors): it starts the
//! worker with one end of a socket pair, sends requests, and waits for replies with a
//! timeout covering both sending and waiting. A worker that crashes, hangs, or replies
//! with something unreadable is killed and reaped, and the call returns a structured
//! error; the app keeps running.
//!
//! Plain threads, no async runtime: one thread reads the worker's replies, and one reads
//! its log lines (JSON, one per line), keeping the last to attach to a crash report, and
//! hands them to the one thread that re-emits every worker's into `tracing` at their own
//! level, so a log sink that falls behind never stops a worker's log being read. Each call
//! has a trace ID, which the worker records on its logs for the request, so they line up
//! with the core's span for the call.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use command_fds::{CommandFdExt, FdMapping};
use silico_protocol::{
    ElementRef, Failure, FailureKind, FrameError, FrameReader, Generation, Op, Refusal, Reply,
    Request, RequestId, Response, TraceId, WORKER_FD, WORKER_FD_ENV, decode, write_message,
};

use crate::references::EdgeNames;
use crate::{Error, ErrorCode};

/// How many of a worker's last log lines a crash report carries.
const STDERR_TAIL_LINES: usize = 20;

/// The longest log line kept; the rest of a longer line is dropped, so a worker can't make
/// the core buffer without limit.
const MAX_LOG_LINE: usize = 8 * 1024;

/// How much of the workers' logs may wait for the log sink, all workers' together, in
/// bytes (each line's message and [`LOG_ITEM_COST`]): a burst of tens of thousands of
/// lines, which a sink slower than the workers catches up on. Past that (a sink that's
/// stalled: a terminal no one reads, a blocked exporter) lines are dropped, counted, and
/// the count logged when the sink catches up, so each worker's log is always read: left
/// unread, it fills the worker's pipe, and the worker blocks writing the line every request
/// ends with, and stops answering, which looks like a hang in the geometry.
const LOG_QUEUE_BYTES: usize = 4 * 1024 * 1024;

/// What each line, or each span let go, costs against [`LOG_QUEUE_BYTES`] besides its
/// message: the item, and its slot in the channel and the allocations it holds, roughly.
const LOG_ITEM_COST: usize = std::mem::size_of::<Queued>() + 64;

/// How long a crash report waits, at most, for the sink to emit the worker's last lines,
/// once they're read: so they're logged before the error that reports them, when the sink
/// keeps up, and a sink that doesn't holds the report up no longer than this.
const LOG_FLUSH_WAIT: Duration = Duration::from_millis(250);

/// How long a worker whose socket closed gets to exit on its own before it's killed, and
/// how long its log then gets to be read.
pub(crate) const EXIT_GRACE: Duration = Duration::from_secs(2);
/// The least a crashed call waits for its worker to exit, past the call's deadline.
const MIN_EXIT_GRACE: Duration = Duration::from_millis(100);

/// The socket buffer size asked for on each end. (silico-kernel's tests/idle.rs sends twice
/// this and a byte, to make a send wait: change it with this.)
const SOCKET_BUFFER: usize = 4 * 1024 * 1024;

/// The only variables a worker gets from the app's environment. OCCT reads others (such
/// as `CSF_MeshAlgo`, which picks the meshing algorithm), so passing the user's shell
/// environment through would make results depend on it.
const WORKER_ENV: &[&str] = &["SILICO_LOG", "RUST_BACKTRACE", "RUST_LIB_BACKTRACE", "PATH"];

/// Trace IDs, unique within this process (across all workers).
static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

/// Held while a worker is started, from making its socket pair until the process is
/// running. On macOS the standard library marks new descriptors close-on-exec only after
/// making them, so a worker started by another thread in that gap would inherit this one's
/// socket, log pipe, or the pipe `Command::spawn` waits on to learn the program started.
/// Workers close what they inherit when they start running (`silico_worker`), so
/// such a descriptor would be held only until then; but until then is as long as the
/// system takes to launch the worker (macOS has held a newly copied binary for 17 s and
/// more), and that spawn would wait as long. Starting workers one at a time closes the gap;
/// it costs a pool of fourteen a few milliseconds at start.
static SPAWNING: Mutex<()> = Mutex::new(());

/// Which kind of worker a [`Worker`] runs, so its errors name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    /// OCCT's (silico-kernel).
    Kernel,
    /// PlaneGCS's (silico-sketch).
    Sketch,
    /// re_renderer's, on lavapipe (silico-render).
    Render,
}

impl WorkerKind {
    /// As errors name it: "kernel worker", "sketch worker", "render worker".
    pub fn noun(self) -> &'static str {
        match self {
            WorkerKind::Kernel => "kernel worker",
            WorkerKind::Sketch => "sketch worker",
            WorkerKind::Render => "render worker",
        }
    }

    /// What the worker runs, as an error names it: "the kernel", "the sketch solver".
    fn runs(self) -> &'static str {
        match self {
            WorkerKind::Kernel => "the kernel",
            WorkerKind::Sketch => "the sketch solver",
            WorkerKind::Render => "the renderer",
        }
    }
}

/// A check of a worker as it starts (see [`Worker::spawn_checked`]).
pub(crate) type StartCheck = Box<dyn FnOnce() -> Result<(), Error> + Send>;

/// A file's size, modification and change times, and inode: what changes when it's rebuilt
/// or replaced. The change time (`st_ctime`) as well as the modification time, since a write
/// can put the modification time back (as `cp -p` onto the file does) but not the change
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    /// The change time: seconds, and nanoseconds past them.
    changed: (i64, i64),
    inode: u64,
}

impl Stamp {
    pub(crate) fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path).ok().map(|meta| Self::from(&meta))
    }
}

impl From<&std::fs::Metadata> for Stamp {
    fn from(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            changed: (meta.ctime(), meta.ctime_nsec()),
            inode: meta.ino(),
        }
    }
}

/// A worker's executable, as it was when first found (before any worker of it starts): its
/// identity (see
/// [`hashed`]), which results kept by key carry, and its stamp then. A worker is started
/// from it, and taken, only while it's still that build ([`Executable::check`]): another
/// would answer under this one's identity with another kernel's (or solver's) results.
pub(crate) struct Executable {
    path: PathBuf,
    kind: WorkerKind,
    identity: String,
    /// The file as it was when last found to be this build: renewed when a change that
    /// keeps the bytes (a relink, a `chmod`) is hashed and found the same, so it's hashed
    /// once for it, not at every start. Its lock is taken alone, never while another is.
    stamp: Mutex<Stamp>,
}

impl Executable {
    /// The executable at `path`, of a `kind` of worker, as it is now.
    pub(crate) fn of(path: &Path, kind: WorkerKind) -> Result<Arc<Self>, Error> {
        let (identity, stamp) = hashed(path, kind)?;
        Ok(Arc::new(Self {
            path: path.to_owned(),
            kind,
            identity,
            stamp: Mutex::new(stamp),
        }))
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    /// Whether the file is still this build. Cheap when it's as it was (its stamp); hashed
    /// again only when that's changed.
    pub(crate) fn check(&self) -> Result<(), Error> {
        let known = *lock(&self.stamp);
        if Stamp::of(&self.path) == Some(known) {
            return Ok(());
        }
        let (identity, stamp) = hashed(&self.path, self.kind)?;
        if identity == self.identity {
            *lock(&self.stamp) = stamp;
            return Ok(());
        }
        let noun = self.kind.noun();
        Err(Error::new(
            ErrorCode::WorkerUnavailable,
            format!(
                "the {noun} {} changed since it started: another build of it isn't started \
                 in its place",
                self.path.display()
            ),
        )
        .with_suggestion(format!(
            "restart Silico, so the new {noun}'s results are kept as its own"
        )))
    }

    /// Starts a worker from the file, if it's still this build, and checks it again once
    /// the worker has started (see [`Worker::spawn_checked`]): a file replaced between the
    /// two checks (around the exec) shows, and that worker isn't trusted. A file replaced
    /// and put back between them would pass: two replacements in the moments a worker takes
    /// to start, out of reach without pinning the executable (an open file to exec, or a
    /// private copy), which waits on how M5 packages the workers.
    pub(crate) fn spawn(self: &Arc<Self>) -> Result<Worker, Error> {
        self.check()?;
        let again = Arc::clone(self);
        Worker::spawn_checked(&self.path, self.kind, Some(Box::new(move || again.check())))
    }
}

/// The identity of the worker executable at `exe`, a `kind` of worker: its noun (`kernel
/// worker`), `sha256:`, and its bytes' SHA-256, so a rebuilt or replaced worker is another
/// kernel or solver. Results kept by key carry it (M2.5's cache keeps the kernel's on disk,
/// so its spelling doesn't change). And the file's stamp, from the same open file, so the
/// two are of one file.
fn hashed(exe: &Path, kind: WorkerKind) -> Result<(String, Stamp), Error> {
    use sha2::{Digest, Sha256};
    let noun = kind.noun();
    // Opened without waiting, and read only if it's a regular file: a FIFO put at the path
    // would block its open (and the pool's thread) until something wrote to it, and a device
    // (a link to /dev/zero) would be read without end.
    let hashed_and_stamped = (|| {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(exe)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it isn't a regular file",
            ));
        }
        let mut hasher = Sha256::new();
        io::copy(&mut file, &mut hasher)?;
        Ok((hasher.finalize(), Stamp::from(&meta)))
    })()
    .map_err(|e: io::Error| {
        Error::new(
            ErrorCode::WorkerUnavailable,
            format!("couldn't read the {noun} {}", exe.display()),
        )
        .with_cause(e.to_string())
        .with_suggestion(format!("check Silico's install: rebuild the {noun}"))
    })?;
    let (digest, stamp) = hashed_and_stamped;
    Ok((format!("{noun} sha256:{}", crate::hex(&digest)), stamp))
}

/// A running worker. Dropping it kills and reaps the process.
pub struct Worker {
    kind: WorkerKind,
    /// Shared with [`WorkerKiller`]s, behind a lock taken for every kill and every reap, so a
    /// killer never signals a process that's been reaped (whose ID may be someone else's).
    child: Arc<Mutex<Child>>,
    socket: UnixStream,
    replies: Receiver<Result<Response, FrameError>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// The requests whose log lines the log thread may still read: each one's trace, and
    /// the span it was asked in, under which its lines go, until the worker's line saying
    /// the request is done ([`silico_protocol::REQUEST_DONE`]) is read.
    callers: Arc<Mutex<Callers>>,
    /// Closes when the log thread has read everything the worker wrote.
    stderr_read: Receiver<()>,
    /// Closes when the log sink thread has emitted everything the log thread gave it (or
    /// at once, if the log can't reach it).
    stderr_emitted: Receiver<()>,
    /// Set when the worker says it has started (see [`silico_protocol::write_started`]), or
    /// replies.
    started: Arc<AtomicBool>,
    /// Why the worker isn't to be trusted, if its start's check found so (see
    /// [`Worker::spawn_checked`]): its calls fail with it, and stop it.
    refused: Arc<std::sync::OnceLock<Error>>,
    /// How many requests the worker has answered (with a result or a refusal).
    answers: u64,
    next_id: u64,
    /// Set once the worker has crashed, hung, or misbehaved and been stopped.
    stopped: bool,
}

impl Worker {
    /// Starts the kernel worker executable at `exe`.
    pub fn spawn(exe: &Path) -> Result<Self, Error> {
        Self::spawn_as(exe, WorkerKind::Kernel)
    }

    /// Starts the worker executable at `exe`, of the given kind.
    pub fn spawn_as(exe: &Path, kind: WorkerKind) -> Result<Self, Error> {
        Self::spawn_checked(exe, kind, None)
    }

    /// Starts the worker executable at `exe`, of the given kind, and, once it has started
    /// (its first frame, sent as its process begins), runs `check`: if that fails, the
    /// worker isn't trusted, and every call fails with its error (and stops it) rather than
    /// taking what it answers. Checked there, close after the exec, so a change made since
    /// a check before the start shows (one put back between the two doesn't: see
    /// [`Executable::spawn`]).
    pub(crate) fn spawn_checked(
        exe: &Path,
        kind: WorkerKind,
        check: Option<StartCheck>,
    ) -> Result<Self, Error> {
        let spawning = lock(&SPAWNING);
        let (ours, theirs) =
            UnixStream::pair().map_err(|e| start_failed(kind, "socket pair", &e))?;
        // Larger buffers than the platform default (8 KiB on macOS), so a multi-megabyte
        // shape moves in few system calls. The kernel may cap them; that's fine.
        for end in [&ours, &theirs] {
            let socket = socket2::SockRef::from(end);
            let _ = socket.set_send_buffer_size(SOCKET_BUFFER);
            let _ = socket.set_recv_buffer_size(SOCKET_BUFFER);
        }
        let mut command = Command::new(exe);
        command.env_clear();
        for name in WORKER_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env(WORKER_FD_ENV, WORKER_FD.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
            .fd_mappings(vec![FdMapping {
                parent_fd: OwnedFd::from(theirs),
                child_fd: WORKER_FD,
            }])
            .map_err(|e| internal(kind, "passing the socket", e))?;
        let child = command.spawn().map_err(|e| {
            let message = format!("couldn't start the {} {}", kind.noun(), exe.display());
            // Short of something, the worker never ran; otherwise it can't be run.
            shortage(&e, &message).unwrap_or_else(|| {
                Error::new(ErrorCode::WorkerUnavailable, message)
                    .with_cause(e.to_string())
                    .with_suggestion(format!(
                        "check Silico's install: {} can't be run (missing, not executable, or \
                         needing an interpreter that's missing): rebuild it",
                        exe.display()
                    ))
            })
        })?;
        // The command (and with it the worker's end of the socket) must go now, or the
        // worker's end stays open here and its exit would never close the socket.
        drop(command);
        drop(spawning);
        // From here on, an early return must not leave the process running.
        let mut guard = KillOnDrop(Some(child));
        let pid = guard.id();

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let callers = Arc::new(Mutex::new(Callers::default()));
        let (read_tx, stderr_read) = mpsc::channel::<()>();
        let (emitted_tx, stderr_emitted) = mpsc::channel::<()>();
        if let Some(stderr) = guard.take_stderr() {
            let queue = LogQueue::shared(pid)
                .map_err(|e| start_failed(kind, "starting the log sink thread", &e))?;
            let tail = Arc::clone(&stderr_tail);
            let callers = Arc::clone(&callers);
            std::thread::Builder::new()
                .name(format!("worker-{pid}-logs"))
                .spawn(move || {
                    forward_logs(stderr, &tail, &callers, &queue);
                    // The spans of requests the worker never said were done (one that timed
                    // out, say) go to the sink thread too: this thread may hold the last
                    // handle on them.
                    let left = std::mem::take(&mut lock(&callers).0);
                    queue.release(left.into_iter().map(|(_, span)| span).collect());
                    drop(read_tx);
                    queue.flushed(emitted_tx);
                })
                .map_err(|e| start_failed(kind, "starting the log thread", &e))?;
        }

        // Non-blocking, so a send can give up at its deadline (see `DeadlineWriter`).
        ours.set_nonblocking(true)
            .map_err(|e| start_failed(kind, "making the socket non-blocking", &e))?;
        let reader = ours
            .try_clone()
            .map_err(|e| start_failed(kind, "socket clone", &e))?;
        // One reply in flight: a worker sending unrequested replies can't make the core
        // buffer them without limit.
        let (tx, replies) = mpsc::sync_channel(1);
        let started = Arc::new(AtomicBool::new(false));
        let has_started = Arc::clone(&started);
        let refused = Arc::new(std::sync::OnceLock::new());
        let refuse = Arc::clone(&refused);
        let mut check = check;
        std::thread::Builder::new()
            .name(format!("worker-{pid}-replies"))
            .spawn(move || {
                let mut frames = FrameReader::new(PatientReader(reader));
                loop {
                    let reply = match frames.read_frame() {
                        // The worker has started (any frame says so); an empty one says only that.
                        // Checked first, so no reply is taken before.
                        Ok(Some(frame)) => {
                            let refused = check.take().and_then(|check| check().err());
                            if let Some(e) = refused {
                                // Not said to have started: a refused worker is no sign the
                                // executable works (the pool's slots would cut their waits).
                                // Nothing more is passed on: the call waiting (or the
                                // next) ends at once, with the refusal, rather than when
                                // the worker answers.
                                let _ = refuse.set(e);
                                break;
                            }
                            has_started.store(true, Ordering::Relaxed);
                            if frame.is_empty() {
                                continue;
                            }
                            decode::<Response>(&frame)
                        }
                        // The socket closed: dropping the sender tells the caller.
                        Ok(None) => break,
                        Err(e) => Err(e),
                    };
                    let failed = reply.is_err();
                    if tx.send(reply).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|e| start_failed(kind, "starting the reply thread", &e))?;

        let child = guard
            .release()
            .ok_or_else(|| internal(kind, "keeping the process", "it was lost"))?;
        tracing::debug!(pid, exe = %exe.display(), "started {}", kind.noun());
        Ok(Self {
            kind,
            child: Arc::new(Mutex::new(child)),
            socket: ours,
            replies,
            stderr_tail,
            callers,
            stderr_read,
            stderr_emitted,
            started,
            refused,
            answers: 0,
            next_id: 0,
            stopped: false,
        })
    }

    /// The worker's process ID.
    pub fn pid(&self) -> u32 {
        lock(&self.child).id()
    }

    /// A handle that kills this worker from another thread, so a call waiting on it ends at
    /// once (as a crash). Safe to use after the worker is gone: it never signals a process
    /// that's been reaped.
    pub fn killer(&self) -> WorkerKiller {
        WorkerKiller(Arc::clone(&self.child))
    }

    /// Whether the process has exited (killed from outside while idle, say), reaping it if
    /// so. A call to it would fail as a crash.
    pub fn has_exited(&mut self) -> bool {
        self.exit_status().is_some()
    }

    /// How the process ended, if it has (reaping it).
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        lock(&self.child).try_wait().ok().flatten()
    }

    /// Whether the worker has said it started (see [`silico_protocol::write_started`]). A
    /// worker that has ended may have said so in frames not yet read: [`Self::has_exited`]
    /// and then [`Self::drain_replies`] first.
    pub(crate) fn has_started(&self) -> bool {
        self.started.load(Ordering::Relaxed)
    }

    /// Blocks until `until`, at the latest, reading and dropping what a worker whose socket
    /// has closed sent before it ended. Once the socket's end is read (it ends at once,
    /// unless a process the worker started holds it open), [`Self::has_started`] is final.
    pub(crate) fn drain_replies(&mut self, until: Instant) {
        while self
            .replies
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .is_ok()
        {}
    }

    /// How many requests the worker has answered, with a result or a refusal: not counting
    /// one refused before it was sent.
    pub(crate) fn answers(&self) -> u64 {
        self.answers
    }

    /// Whether the worker can't be given a request: it has sent something no call asked for
    /// (a second reply to an earlier request, say, which is read and dropped: its next call
    /// would fail on it, out of turn), or closed its socket, though it may still be running.
    pub(crate) fn has_hung_up(&mut self) -> bool {
        !matches!(self.replies.try_recv(), Err(TryRecvError::Empty))
    }

    /// How the process ended, waiting up to `grace` for it to (reaping it).
    pub(crate) fn exit_status_within(&mut self, grace: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = self.exit_status() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Stops the worker (kills and reaps it); later calls fail at once. For a worker whose
    /// replies can't be trusted.
    pub fn shut_down(&mut self) {
        kill_and_reap(&self.child);
        self.stopped = true;
    }

    /// How many requests have been sent to this worker (or tried). For tests, which check
    /// that a refused value never reached it.
    pub fn requests_sent(&self) -> u64 {
        self.next_id
    }

    /// Whether the worker has been stopped after a crash, hang, or bad reply.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Sends `op` and waits for the reply. `timeout` covers both sending the request and
    /// waiting for the reply.
    pub fn call(
        &mut self,
        generation: Generation,
        op: Op,
        timeout: Duration,
    ) -> Result<Reply, Error> {
        self.call_with_edge_names(generation, op, timeout, None)
    }

    /// [`call`](Self::call), a refusal's edges named in its error as `names` says, when it
    /// says (a document's fillet or chamfer, whose edges are its names).
    pub(crate) fn call_with_edge_names(
        &mut self,
        generation: Generation,
        op: Op,
        timeout: Duration,
        names: Option<&EdgeNames>,
    ) -> Result<Reply, Error> {
        let name = op.name();
        // The edges a refusal may name: those the request lists.
        let listed = match &op {
            Op::Fillet { edges, .. } | Op::Chamfer { edges, .. } => Some(edges.clone()),
            _ => None,
        };
        if self.stopped {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                format!(
                    "can't send {name}: this {} was stopped after an earlier failure",
                    self.kind.noun()
                ),
            )
            .with_suggestion(format!("start a new {}", self.kind.noun())));
        }
        // A worker its start's check refused isn't sent anything.
        if let Some(refused) = self.refused.get() {
            return Err(self.stop(refused.clone()));
        }
        let id = RequestId(self.next_id);
        self.next_id += 1;
        let trace = TraceId(NEXT_TRACE.fetch_add(1, Ordering::Relaxed));
        // The worker's log lines for this request go under the span it was asked in, so at
        // any level they trace back to it (ARCHITECTURE.md, invariant 9). The log sink runs
        // only outside the lock, which the log thread takes for every line: taking the span,
        // and warning of and letting go of one pushed out.
        let asked_in = tracing::Span::current();
        let pushed_out = lock(&self.callers).push(trace.0, asked_in);
        pushed_out_of_callers(pushed_out);
        let span = tracing::debug_span!(
            "worker_call",
            trace = trace.0,
            pid = self.pid(),
            id = id.0,
            generation = generation.0,
            op = name
        );
        let _entered = span.enter();
        // From here, after the call's own logging, which a slow sink can make slow: a timeout
        // too long to add to the clock has no deadline.
        let deadline = Instant::now().checked_add(timeout);

        let request = Request {
            id,
            generation,
            trace,
            op,
        };
        // A send that can't finish in time (a worker that stopped reading, or reads slowly)
        // is a timeout.
        let mut writer = DeadlineWriter {
            socket: &self.socket,
            deadline,
            sent: 0,
        };
        let written = write_message(&mut writer, &request);
        // A worker that took any of the request may have died of it (reading a large one,
        // say); one that took none was gone before it.
        let reached = writer.sent > 0;
        match written {
            Ok(()) => {}
            // Refused before anything was sent: the request, not the worker, is at fault.
            Err(e @ (FrameError::TooLarge { .. } | FrameError::Encode(_))) => {
                // The worker will never say it's done: its span goes now.
                // Bound first, so it's let go once the lock is (see above).
                let forgotten = lock(&self.callers).forget(trace.0);
                drop(forgotten);
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!("{name} couldn't be sent to the {}", self.kind.noun()),
                )
                .with_cause(e.to_string())
                .with_suggestion("make the request smaller: send smaller shapes"));
            }
            Err(FrameError::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(self.timed_out(name, timeout));
            }
            Err(e) => return Err(self.crashed(name, deadline, reached, Some(e.to_string()))),
        }
        tracing::trace!("sent");

        let reply = match deadline {
            Some(deadline) => self
                .replies
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => self
                .replies
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };
        // The start's check runs before any reply is passed on, so it has run by now if
        // the worker started; a refusal ends the wait at once (the reply thread stops).
        if let Some(refused) = self.refused.get() {
            return Err(self.stop(refused.clone()));
        }
        match reply {
            Ok(Ok(response)) if response.id == id && response.generation == generation => {
                self.answers += 1;
                match response.result {
                    Ok(reply) => Ok(reply),
                    // Checked before it's described, so a refusal the request can't explain
                    // (a broken worker's) costs no more than the request does.
                    Err(failure) => match refusal_fits(&failure.refusal, listed.as_deref()) {
                        Ok(()) => Err(failure_error_with_edge_names(
                            self.kind, name, failure, names,
                        )),
                        Err(misfit) => Err(self.stop(
                            Error::new(
                                ErrorCode::ProtocolError,
                                format!(
                                    "the {}'s refusal of {name} doesn't fit the request",
                                    self.kind.noun()
                                ),
                            )
                            .with_cause(misfit)
                            .with_suggestion(broken_worker(self.kind)),
                        )),
                    },
                }
            }
            Ok(Ok(response)) => Err(self.stop(
                Error::new(
                    ErrorCode::ProtocolError,
                    format!("the {} answered {name} out of turn", self.kind.noun()),
                )
                .with_cause(format!(
                    "reply for request {} (generation {}), expected {} (generation {})",
                    response.id.0, response.generation.0, id.0, generation.0
                ))
                .with_suggestion(broken_worker(self.kind)),
            )),
            Ok(Err(FrameError::Io(_) | FrameError::Truncated { .. }))
            | Err(RecvTimeoutError::Disconnected) => Err(self.crashed(name, deadline, true, None)),
            Ok(Err(e)) => Err(self.stop(
                Error::new(
                    ErrorCode::ProtocolError,
                    format!(
                        "the {}'s reply to {name} couldn't be read",
                        self.kind.noun()
                    ),
                )
                .with_cause(e.to_string())
                .with_suggestion(broken_worker(self.kind)),
            )),
            Err(RecvTimeoutError::Timeout) => Err(self.timed_out(name, timeout)),
        }
    }

    fn timed_out(&mut self, name: &str, timeout: Duration) -> Error {
        let error = Error::new(
            ErrorCode::WorkerTimeout,
            format!(
                "the {} didn't finish {name} within {timeout:?}",
                self.kind.noun()
            ),
        );
        // A worker that hadn't started never had the request to work on.
        let error = if self.has_started() {
            error.with_suggestion("simplify the operation, or allow it more time")
        } else {
            error
                .with_cause(format!("the {} hadn't started", self.kind.noun()))
                .with_suggestion(format!(
                    "try the request again: the {} hadn't started in the time allowed (a new \
                     worker binary's first launch can be slow while the system checks it); if \
                     it never starts, check Silico's install",
                    self.kind.noun()
                ))
        };
        self.stop(error)
    }

    /// The worker's socket closed: report how it ended, with its last log lines.
    fn crashed(
        &mut self,
        name: &str,
        deadline: Option<Instant>,
        took_request: bool,
        write_error: Option<String>,
    ) -> Error {
        // Within the call's time, if it's sooner, but long enough to reap a worker that's
        // ending: one that closed its socket and lingers is killed then. Its replies and its
        // log are read until then too (a process it started may hold them open).
        let grace = deadline.map_or(EXIT_GRACE, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .clamp(MIN_EXIT_GRACE, EXIT_GRACE)
        });
        tracing::debug!(
            grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX),
            "waiting for a worker whose socket closed to exit before it's killed"
        );
        let until = Instant::now() + grace;
        let status = self.wait_for_exit(grace);
        // What the worker sent before it ended may say it started.
        self.drain_replies(until);
        let reached = match (self.has_started(), took_request) {
            (false, _) => Reached::NotStarted,
            (true, false) => Reached::Started,
            (true, true) => Reached::Request,
        };
        // The worker has exited, so its log pipe closes; let the log thread read the last
        // lines before the report is built (the tail is kept as they're read). And give the
        // sink a moment to emit them, so they're logged before the error that reports them;
        // a sink that's behind or stalled doesn't hold the report, and the slot, up longer.
        let _ = self
            .stderr_read
            .recv_timeout(until.saturating_duration_since(Instant::now()));
        let flushed = until.min(Instant::now() + LOG_FLUSH_WAIT);
        let _ = self
            .stderr_emitted
            .recv_timeout(flushed.saturating_duration_since(Instant::now()));
        let mut cause = match status {
            Some(status) => describe_exit(status),
            None => "it stopped responding and was killed".to_owned(),
        };
        if let Some(e) = write_error {
            cause = format!("{cause} (sending the request failed: {e})");
        }
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !tail.is_empty() {
            let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
            cause = format!("{cause}; its last output:\n{}", lines.join("\n"));
        }
        drop(tail);
        self.stopped = true;
        // One that ended before its first frame never was a worker of this build: it can't
        // be had (a missing library, an exit at launch), not a crash of one that ran.
        let noun = self.kind.noun();
        let (code, message) = if self.has_started() {
            (
                ErrorCode::WorkerCrashed,
                format!("the {noun} crashed during {name}"),
            )
        } else {
            (
                ErrorCode::WorkerUnavailable,
                format!("the {noun} ended before it started, so it never ran {name}"),
            )
        };
        Error::new(code, message)
            .with_cause(cause)
            .with_suggestion(crash_advice(self.kind, status, reached))
    }

    /// Waits briefly for a worker whose socket closed to exit, then kills it if needed.
    fn wait_for_exit(&mut self, grace: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            match lock(&self.child).try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {}
                Err(_) => break,
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        kill_and_reap(&self.child);
        None
    }

    /// Kills and reaps the worker, and returns `error`.
    fn stop(&mut self, error: Error) -> Error {
        kill_and_reap(&self.child);
        self.stopped = true;
        error
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        kill_and_reap(&self.child);
    }
}

/// Kills a [`Worker`] from another thread (see [`Worker::killer`]).
#[derive(Clone)]
pub struct WorkerKiller(Arc<Mutex<Child>>);

impl WorkerKiller {
    /// Kills the worker if it's still running, and says whether it was: false if it had
    /// already exited, on its own or killed before, so that its call's failure isn't this
    /// kill's doing. Its call, if one is waiting, ends as a crash; the worker's owner reaps it
    /// (or finds it reaped here, if it had exited).
    pub fn kill(&self) -> bool {
        let mut child = lock(&self.0);
        // Once the child has been waited for its status is kept, and `Child::kill` signals
        // nothing, so a process ID the system has given to another process is never signalled.
        match child.try_wait() {
            Ok(None) => child.kill().is_ok(),
            Ok(Some(_)) | Err(_) => false,
        }
    }

    /// The worker's process ID.
    pub fn pid(&self) -> u32 {
        lock(&self.0).id()
    }
}

/// Locks `mutex`, carrying on if a thread panicked holding it: what it guards (a process, a
/// queue) is still the truth, and the core must not panic on a worker's account.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes to the worker's (non-blocking) socket, giving up when `deadline` passes. A
/// socket's write timeout can't do this: on macOS it limits each wait for buffer space, not
/// the write, so a worker that reads a little at a time could keep a write going without
/// end. Instead each write sends what fits, and waits for space with `poll`.
struct DeadlineWriter<'a> {
    socket: &'a UnixStream,
    deadline: Option<Instant>,
    /// Bytes the worker's socket has taken so far.
    sent: usize,
}

impl io::Write for DeadlineWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            match (&*self.socket).write(bytes) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Ok(n) => {
                    self.sent += n;
                    return Ok(n);
                }
                result => return result,
            }
            let left = match self.deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    Some(left)
                }
                None => None,
            };
            wait_for(self.socket, rustix::event::PollFlags::OUT, left)?;
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads from the worker's (non-blocking) socket, waiting as long as it takes.
struct PatientReader(UnixStream);

impl Read for PatientReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.0.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                result => return result,
            }
            wait_for(&self.0, rustix::event::PollFlags::IN, None)?;
        }
    }
}

/// Waits until `socket` is ready for `events` (or has closed), or `timeout` passes.
fn wait_for(
    socket: &UnixStream,
    events: rustix::event::PollFlags,
    timeout: Option<Duration>,
) -> io::Result<()> {
    use rustix::event::{PollFd, Timespec, poll};
    // poll takes whole milliseconds as a C int on macOS, so a longer wait is refused (about
    // 24.8 days). Waiting at most a day at a time is the same: the callers wait again.
    const LONGEST_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
    let timeout = timeout.map(|t| t.min(LONGEST_WAIT)).map(|t| Timespec {
        tv_sec: i64::try_from(t.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(t.subsec_nanos()),
    });
    let mut fds = [PollFd::new(socket, events)];
    match poll(&mut fds, timeout.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Kills and reaps a worker that a [`WorkerKiller`] on another thread may also hold.
fn kill_and_reap(child: &Mutex<Child>) {
    // Does nothing once the process has been reaped (and the wait below then returns its
    // kept status at once); if the signal fails otherwise, there's nothing else to do.
    let _ = lock(child).kill();
    // Waited for without holding the lock (a large process takes a while to go: about 70 ms
    // for 4 GB), so a killer on another thread isn't held up meanwhile; polled, since a
    // blocking wait would need the lock throughout.
    loop {
        match lock(child).try_wait() {
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Ok(Some(_)) | Err(_) => return,
        }
    }
}

/// Kills and reaps a process nothing else holds, so its wait can block.
fn kill_and_reap_child(child: &mut Child) {
    // There's nothing else to do if either fails.
    let _ = child.kill();
    let _ = child.wait();
}

/// Holds a just-started worker so that an early return kills and reaps it.
struct KillOnDrop(Option<Child>);

impl KillOnDrop {
    fn id(&self) -> u32 {
        self.0.as_ref().map_or(0, Child::id)
    }

    fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.0.as_mut().and_then(|child| child.stderr.take())
    }

    /// Hands the process over; dropping the guard then does nothing.
    fn release(mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            kill_and_reap_child(child);
        }
    }
}

/// The spans of a worker's requests whose log lines may still be read, by trace, oldest
/// first. How many it holds is how far the log thread is behind the worker: it doesn't
/// wait on the log sink, but a busy machine can put it behind, as far as the pipe holds
/// done lines (its size over a done line's 28 bytes or so: about 2,300 for a 64 KiB pipe,
/// on macOS and on Linux with 4 KiB pages; more with larger pages), before the worker blocks
/// writing one. [`Callers::MOST`] is above that on the platforms Silico is checked on (a
/// test measures each one's pipe); past it, the oldest span goes (with a warning), and its
/// late lines go under none. It also bounds a worker that never says a request is done.
#[derive(Default)]
struct Callers(VecDeque<(u64, tracing::Span)>);

/// Warns of a span pushed out of [`Callers`], and lets it go: outside its lock.
fn pushed_out_of_callers(span: Option<tracing::Span>) {
    if span.is_some() {
        tracing::warn!(
            "a worker hasn't said {} requests were done: its log lines for the oldest no \
             longer trace back to them",
            Callers::MOST
        );
    }
}

impl Callers {
    const MOST: usize = 4096;

    /// Holds request `trace`'s span, giving back the oldest if it's pushed out. Each method
    /// gives back the spans it lets go, to be dropped outside the lock: closing a span runs
    /// the log sink, which may block.
    fn push(&mut self, trace: u64, span: tracing::Span) -> Option<tracing::Span> {
        let pushed_out = if self.0.len() == Self::MOST {
            self.0.pop_front().map(|(_, span)| span)
        } else {
            None
        };
        self.0.push_back((trace, span));
        pushed_out
    }

    /// Lets the span of request `trace` go, the worker never having been sent it.
    fn forget(&mut self, trace: u64) -> Option<tracing::Span> {
        let at = self.0.iter().position(|(asked, _)| *asked == trace)?;
        self.0.remove(at).map(|(_, span)| span)
    }

    /// The span of request `trace`, held by its lines until they're emitted.
    fn parent(&self, trace: Option<u64>) -> Option<tracing::Span> {
        self.0
            .iter()
            .find(|(asked, _)| Some(*asked) == trace)
            .map(|(_, span)| span.clone())
    }

    /// Lets the span of request `trace` go, and any older (whose lines came before).
    fn done(&mut self, trace: u64) -> Vec<tracing::Span> {
        match self.0.iter().position(|(asked, _)| *asked == trace) {
            Some(at) => self.0.drain(..=at).map(|(_, span)| span).collect(),
            None => Vec::new(),
        }
    }
}

/// Reads the worker's log lines until it closes its end, keeping the last
/// [`STDERR_TAIL_LINES`] for crash reports and passing each to the sink through `queue`,
/// never waiting on it. Lines are read as bytes, so text that isn't UTF-8 can't stop the
/// reading, and cut at [`MAX_LOG_LINE`].
fn forward_logs(
    stderr: impl Read,
    tail: &Mutex<VecDeque<String>>,
    callers: &Mutex<Callers>,
    queue: &LogQueue,
) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_LOG_LINE as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() == MAX_LOG_LINE && !line.ends_with(b"\n") {
            // Cut: drop the rest of the line, so it doesn't arrive as more lines.
            let _ = reader.skip_until(b'\n');
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        match read_worker_log(text, callers) {
            ReadLine::Line(read, summary) => {
                let mut kept = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if kept.len() == STDERR_TAIL_LINES {
                    kept.pop_front();
                }
                kept.push_back(summary);
                drop(kept);
                queue.offer_line(read);
            }
            ReadLine::Done(spans) => queue.release(spans),
        }
    }
}

/// A worker's log thread's side of the hand-off to the sink thread, which emits every
/// worker's lines: it never blocks. What the sink thread hasn't emitted yet is bounded in
/// bytes, all workers' together; a line with no room is dropped and counted, and the first
/// line dropped since the count was last said puts a mark in the queue, where the sink
/// thread says the count: every line dropped by then, each said once.
struct LogQueue {
    pid: u32,
    items: mpsc::Sender<Queued>,
    /// What the queue holds, in bytes (see [`Queued::cost`]), shared by every worker's.
    held: Arc<AtomicUsize>,
    most: usize,
    /// This worker's lines dropped and not yet said. While it's above zero, the queue
    /// holds one [`LogItem::Dropped`] of this worker's, which says them.
    dropped: Arc<AtomicU64>,
}

/// The sink thread every worker's log goes to, started with the first worker: one, so a
/// sink that never recovers holds one thread and one [`LOG_QUEUE_BYTES`], however many
/// workers the pool starts. Its channel and the bytes it holds.
static SINK: Mutex<Option<(mpsc::Sender<Queued>, Arc<AtomicUsize>)>> = Mutex::new(None);

/// What a worker's log thread hands the sink thread.
struct Queued {
    pid: u32,
    item: LogItem,
}

/// One thing in a worker's log for the sink thread: a line, or a mark in the log.
enum LogItem {
    Line(WorkerLine),
    /// Lines were dropped: say how many (the count's the worker's).
    Dropped(Arc<AtomicU64>),
    /// Requests' spans the log thread let go: closing one runs the log sink, which may
    /// block, so never on the log thread. Always queued, though they count against the
    /// bound (so lines give way to them): at worst one for each request the core asked,
    /// under a span the log sink records, while the sink was stalled.
    Release(Vec<tracing::Span>),
    /// The worker's log is all read: dropping this says everything before it is emitted.
    Flushed(mpsc::Sender<()>),
}

impl Queued {
    fn cost(&self) -> usize {
        match &self.item {
            LogItem::Line(line) => line.message.len() + LOG_ITEM_COST,
            LogItem::Release(spans) => spans.len() * LOG_ITEM_COST,
            LogItem::Dropped(_) | LogItem::Flushed(_) => 0,
        }
    }
}

/// The sink thread's work on one item: emits its line, or says its count, or lets its
/// spans go, and gives its bytes back to `held`. A log layer that panics loses the item,
/// not every worker's log after it, nor its bytes; and its spans are let go one at a time,
/// each caught, since a panic closing one would otherwise panic again, unwinding, closing
/// the next, which aborts.
fn sink_one(queued: Queued, held: &AtomicUsize) {
    fn caught(f: impl FnOnce()) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    }
    let cost = queued.cost();
    let pid = queued.pid;
    let spans = match queued.item {
        LogItem::Line(mut line) => {
            caught(|| line.emit(pid));
            line.parent.take().into_iter().collect()
        }
        LogItem::Dropped(count) => {
            caught(|| {
                let dropped = count.swap(0, Ordering::SeqCst);
                tracing::warn!(
                    target: "silico::worker",
                    pid,
                    dropped,
                    "{dropped} of the worker's log lines were dropped: the log sink fell behind"
                );
            });
            Vec::new()
        }
        LogItem::Release(spans) => spans,
        LogItem::Flushed(emitted) => {
            drop(emitted);
            Vec::new()
        }
    };
    for span in spans {
        caught(|| drop(span));
    }
    held.fetch_sub(cost, Ordering::SeqCst);
}

impl LogQueue {
    /// Worker `pid`'s queue to the sink thread every worker's log goes to, starting it if
    /// it isn't running yet.
    fn shared(pid: u32) -> io::Result<Self> {
        let mut sink = lock(&SINK);
        let (items, held) = match &*sink {
            Some(running) => running.clone(),
            None => {
                let (items, queued) = mpsc::channel::<Queued>();
                let held = Arc::new(AtomicUsize::new(0));
                let emitted = Arc::clone(&held);
                std::thread::Builder::new()
                    .name("silico-worker-log-sink".to_owned())
                    .spawn(move || {
                        for item in queued {
                            sink_one(item, &emitted);
                        }
                    })?;
                sink.insert((items, held)).clone()
            }
        };
        Ok(Self::new(pid, items, held, LOG_QUEUE_BYTES))
    }

    fn new(pid: u32, items: mpsc::Sender<Queued>, held: Arc<AtomicUsize>, most: usize) -> Self {
        Self {
            pid,
            items,
            held,
            most,
            dropped: Arc::default(),
        }
    }

    fn send(&self, item: LogItem) -> Result<(), Queued> {
        let queued = Queued {
            pid: self.pid,
            item,
        };
        self.items.send(queued).map_err(|failed| failed.0)
    }

    /// Queues `line` if there's room for it, or drops and counts it.
    fn offer_line(&self, line: WorkerLine) {
        let cost = line.message.len() + LOG_ITEM_COST;
        let held = self.held.fetch_add(cost, Ordering::SeqCst);
        if held + cost <= self.most && self.send(LogItem::Line(line)).is_ok() {
            return;
        }
        self.held.fetch_sub(cost, Ordering::SeqCst);
        // The first line dropped since the count was last said marks where to say it; the
        // mark costs nothing, and there's only ever one of this worker's in the queue.
        if self.dropped.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = self.send(LogItem::Dropped(Arc::clone(&self.dropped)));
        }
    }

    /// Hands the sink thread `spans` to let go of: room or not (see [`LogItem::Release`]).
    /// Those the log sink doesn't record (at the default level, most requests' spans) close
    /// running nothing, and are let go here.
    fn release(&self, mut spans: Vec<tracing::Span>) {
        spans.retain(|span| !span.is_disabled());
        if spans.is_empty() {
            return;
        }
        let cost = spans.len() * LOG_ITEM_COST;
        self.held.fetch_add(cost, Ordering::SeqCst);
        if let Err(unsent) = self.send(LogItem::Release(spans)) {
            // The sink thread has gone (only in tests: the shared one never ends). Its
            // spans can't be let go on this thread, which mustn't block: they're kept.
            self.held.fetch_sub(cost, Ordering::SeqCst);
            std::mem::forget(unsent);
        }
    }

    /// Says, through `emitted`, when the sink thread has emitted everything before now.
    fn flushed(&self, emitted: mpsc::Sender<()>) {
        let _ = self.send(LogItem::Flushed(emitted));
    }
}

/// A worker's log line as the sink thread emits it: its level (none if it isn't one of the
/// worker's `tracing` lines, a crash message from the C runtime, say, emitted as it is at
/// warn), its message, and the span of the request it's for, which it holds until it's
/// emitted.
struct WorkerLine {
    level: Option<tracing::Level>,
    message: String,
    trace: Option<u64>,
    parent: Option<tracing::Span>,
}

impl WorkerLine {
    /// Emits the line at its own level, under its request's span.
    fn emit(&self, pid: u32) {
        let message = &self.message;
        let trace = self.trace;
        let Some(level) = self.level else {
            tracing::warn!(target: "silico::worker", pid, "{message}");
            return;
        };
        let parent = self.parent.as_ref().and_then(tracing::Span::id);
        match level {
            tracing::Level::ERROR => {
                tracing::error!(target: "silico::worker", parent: parent, pid, trace, "{message}");
            }
            tracing::Level::WARN => {
                tracing::warn!(target: "silico::worker", parent: parent, pid, trace, "{message}");
            }
            tracing::Level::INFO => {
                tracing::info!(target: "silico::worker", parent: parent, pid, trace, "{message}");
            }
            tracing::Level::DEBUG => {
                tracing::debug!(target: "silico::worker", parent: parent, pid, trace, "{message}");
            }
            tracing::Level::TRACE => {
                tracing::trace!(target: "silico::worker", parent: parent, pid, trace, "{message}");
            }
        }
    }
}

/// Reads one worker log line (JSON, from `tracing_subscriber`'s json format): its level,
/// its message, and the span its request was asked in (in `callers`, by the line's trace),
/// and gives it with its summary for crash reports, `LEVEL message`. Anything that isn't
/// such a line (a crash message from the C runtime, say) is kept as it is. The line saying
/// a request is done lets its span go (and any older), and isn't a log line.
fn read_worker_log(line: &str, callers: &Mutex<Callers>) -> ReadLine {
    let parsed: Option<serde_json::Value> = serde_json::from_str(line).ok();
    let Some(event) = parsed.filter(serde_json::Value::is_object) else {
        let raw = WorkerLine {
            level: None,
            message: line.to_owned(),
            trace: None,
            parent: None,
        };
        return ReadLine::Line(raw, line.to_owned());
    };
    if let Some(trace) = event[silico_protocol::REQUEST_DONE].as_u64() {
        let spans = lock(callers).done(trace);
        return ReadLine::Done(spans);
    }
    let label = event["level"].as_str().unwrap_or("INFO");
    let level = match label {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        "INFO" => tracing::Level::INFO,
        "DEBUG" => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    let message = event["fields"]["message"].as_str().unwrap_or(line);
    let summary = format!("{label} {message}");
    let trace = event["span"]["trace"].as_u64();
    let parent = lock(callers).parent(trace);
    let read = WorkerLine {
        level: Some(level),
        message: message.to_owned(),
        trace,
        parent,
    };
    ReadLine::Line(read, summary)
}

/// What the log thread reads in a worker's line.
enum ReadLine {
    /// A log line, with its summary for crash reports.
    Line(WorkerLine, String),
    /// The line saying a request is done: the spans it let go.
    Done(Vec<tracing::Span>),
}

/// Signals a worker gets from a fault of its own. Each is named in `signal_name` too.
const FAULTS: [i32; 7] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGABRT,
    libc::SIGTRAP,
    libc::SIGSYS,
];

/// How far a worker got before it ended.
#[derive(Clone, Copy, Debug)]
enum Reached {
    /// It never said it had started.
    NotStarted,
    /// It started, but none of the request reached it.
    Started,
    /// Some of the request reached it (its socket took some, at least).
    Request,
}

/// What to try after a worker crashed, by how it ended, and how far it got. A fault of its
/// own (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT, SIGTRAP, SIGSYS) before it started is
/// its install (a missing library: dyld aborts it), and once the request reached it is the
/// request's values, which would crash it again. SIGKILL is usually the system's, when it
/// runs out of memory, and a request that reached it may be what used the memory: worth
/// one more try; before it started, it may be the system refusing the binary, if it
/// happens again. Any other signal (SIGTERM, SIGINT, SIGHUP: a kill, a Ctrl-C, a
/// shutdown), a worker that stopped responding, or one gone before the request reached it
/// is from outside, worth trying again (the next request starts another worker). A worker
/// that exited with a code ended on its own, which is its install, not the request.
fn crash_advice(kind: WorkerKind, status: Option<ExitStatus>, reached: Reached) -> String {
    let request = matches!(reached, Reached::Request);
    let worker = kind.noun();
    match status.map(|s| (s.code(), s.signal())) {
        Some((Some(_), _)) => format!(
            "check Silico's install: the {worker} exited on its own (see its last output), \
             which no change to the request will help; report it if it persists"
        ),
        Some((None, Some(signal)))
            if matches!(reached, Reached::NotStarted) && FAULTS.contains(&signal) =>
        {
            format!(
                "check Silico's install: the {worker} crashed as it started, before it could \
                 read the request (see its last output); report it if it persists"
            )
        }
        Some((None, Some(signal))) if request && FAULTS.contains(&signal) => format!(
            "change the operation's values: these crashed {}, and the same request would \
             crash it again",
            kind.runs()
        ),
        Some((None, Some(libc::SIGKILL))) if matches!(reached, Reached::NotStarted) => format!(
            "try the request again: the {worker} was killed before it started; if it's killed \
             before starting again, check Silico's install (macOS kills a worker binary whose \
             code signature is broken)"
        ),
        Some((None, Some(libc::SIGKILL))) if request => format!(
            "try the request once more: the {worker} was killed, most likely by the system for \
             running out of memory; if it's killed again, the request needs too much memory: \
             simplify it"
        ),
        _ => format!(
            "try the request again: the {worker} was stopped from outside, stopped responding, \
             or was gone before the request reached it, not by these values"
        ),
    }
}

/// How the worker process ended, naming common signals by this platform's numbering.
fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("it exited with code {code}"),
        (None, Some(signal)) => match signal_name(signal) {
            Some(name) => format!("it was killed by signal {signal} ({name})"),
            None => format!("it was killed by signal {signal}"),
        },
        (None, None) => "it ended for an unknown reason".to_owned(),
    }
}

fn signal_name(signal: i32) -> Option<&'static str> {
    [
        (libc::SIGILL, "SIGILL"),
        (libc::SIGABRT, "SIGABRT"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGKILL, "SIGKILL"),
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGTRAP, "SIGTRAP"),
        (libc::SIGSYS, "SIGSYS"),
    ]
    .into_iter()
    .find(|(number, _)| *number == signal)
    .map(|(_, name)| name)
}

/// What to try when the worker itself misbehaved: nothing about the request will help.
fn broken_worker(kind: WorkerKind) -> String {
    format!(
        "this is a bug in Silico's {}, not in the request: report it (the worker was \
         stopped, and the next request starts another)",
        kind.noun()
    )
}

/// What to try after an internal error: a bug, not the request's values.
pub const SILICO_BUG: &str = "this is a bug in Silico, not in the request: report it";

/// Whether a refusal fits the request it answers (`listed`, the edges a fillet or chamfer
/// lists): every edge it names is one the request lists, each once and in increasing
/// order, so it names no more than the request does; a refusal of edges names some; a
/// request that lists none has none named; a face is numbered from 1. Why not, if not.
fn refusal_fits(refusal: &Refusal, listed: Option<&[u32]>) -> Result<(), String> {
    let named: &[u32] = match refusal {
        Refusal::Occt { edges, .. } | Refusal::ResultInvalid { edges, .. } => edges,
        Refusal::EdgeMissing { edge, .. } | Refusal::EdgeTwice { edge } => {
            std::slice::from_ref(edge)
        }
        Refusal::EdgesRefused { troubles, .. } => {
            if troubles.is_empty() {
                return Err("it refuses edges, but names none".to_owned());
            }
            let edges: Vec<u32> = troubles.iter().map(|t| t.edge).collect();
            return edges_fit(&edges, listed);
        }
        Refusal::EdgesFailed { edges, .. } => {
            if edges.is_empty() {
                return Err("it says OCCT couldn't build edges, but names none".to_owned());
            }
            edges
        }
        Refusal::NoEdges { .. } => {
            return match listed {
                Some([]) => Ok(()),
                _ => Err("it says no edges were listed, but some were".to_owned()),
            };
        }
        Refusal::FacesUnmeshed { .. } => {
            return Err(
                "it refuses a mesh for its unmeshed faces, which only the core decides".to_owned(),
            );
        }
        _ => &[],
    };
    edges_fit(named, listed)
}

/// Whether `named` edges are among those `listed`, each once, in increasing order. An
/// edge the request lists twice, or one it lists that doesn't exist, may be named once.
fn edges_fit(named: &[u32], listed: Option<&[u32]>) -> Result<(), String> {
    if named.is_empty() {
        return Ok(());
    }
    let Some(listed) = listed else {
        return Err("it names edges, but the request lists none".to_owned());
    };
    if !named.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err("it names an edge twice, or out of order".to_owned());
    }
    let listed: std::collections::BTreeSet<u32> = listed.iter().copied().collect();
    match named.iter().find(|edge| !listed.contains(edge)) {
        Some(edge) => Err(format!(
            "it names edge {edge}, which the request doesn't list"
        )),
        None => Ok(()),
    }
}

/// The operation whose refusals an error keeps as data: a measure's, which the rebuild
/// cache keeps (`Op::Summarize`'s name).
const MEASURE: &str = "Summarize";

/// A worker's refusal or failure, as a structured error. The core names the elements at
/// fault (as FreeCAD does, `Edge3`, where no document names them: `references::EdgeNames`) and has
/// the refusal described with those names (`Refusal::describe`). A kernel failure always
/// suggests something.
pub(crate) fn failure_error(op: &str, failure: Failure) -> Error {
    failure_error_with_edge_names(WorkerKind::Kernel, op, failure, None)
}

/// [`failure_error`] for a worker of the given kind, the edges `names` names (if it's
/// given) named as the document does: each by its place in `edges` in the words, by its
/// name among the elements, and the parameter that place, if every edge at fault has one.
fn failure_error_with_edge_names(
    kind: WorkerKind,
    op: &str,
    failure: Failure,
    names: Option<&EdgeNames>,
) -> Error {
    let named = |element: ElementRef| match (element.kind, names) {
        (silico_protocol::ElementKind::Edge, Some(names)) => names.get(element.index),
        _ => None,
    };
    let name = |element: ElementRef| match named(element) {
        Some((position, _)) => format!("edges.{position}"),
        None => element.to_string(),
    };
    let element_name = |element: ElementRef| match named(element) {
        // As written, cut short as a broken reference's is (`references::resolve`).
        Some((_, written)) => crate::references::cut_short(written),
        None => element.to_string(),
    };
    // A kernel worker's refusal of a measure is kept with the error, so the rebuild cache
    // keeps it as data, for the core that reads it to word (no other error is cached).
    let words =
        (kind == WorkerKind::Kernel && op == MEASURE && names.is_none()).then(|| failure.clone());
    let described = failure.refusal.describe(failure.cause.as_deref(), &name);
    // Why it failed: OCCT's words where it threw; otherwise the refusal's.
    let why = failure
        .cause
        .clone()
        .unwrap_or_else(|| described.message.clone());
    let (error, fallback) = match failure.kind() {
        FailureKind::InvalidInput => {
            let error = Error::new(ErrorCode::InvalidInput, described.message);
            (
                match failure.cause {
                    Some(cause) => error.with_cause(cause),
                    None => error,
                },
                "change the value the message names",
            )
        }
        FailureKind::KernelFailed => (
            Error::new(ErrorCode::KernelFailed, format!("{op} failed")).with_cause(why),
            "change the operation's values: the kernel can't build it from these",
        ),
        FailureKind::InvalidResult => (
            Error::new(
                ErrorCode::InvalidResult,
                format!("{op} gave an invalid result, which was refused"),
            )
            .with_cause(described.message),
            "change the operation's values: the kernel can't build a valid result from these",
        ),
        FailureKind::ResourceExhausted => (
            Error::new(
                ErrorCode::ResourceExhausted,
                format!("{op} ran out of memory"),
            )
            .with_cause(why),
            silico_protocol::OUT_OF_MEMORY,
        ),
        FailureKind::Unavailable => (
            Error::new(
                ErrorCode::WorkerUnavailable,
                format!("the {} can't do {op} on this machine", kind.noun()),
            )
            .with_cause(described.message),
            "set up the machine as the cause says, or ask for something else",
        ),
        FailureKind::UnknownOp | FailureKind::Internal => (
            Error::new(
                ErrorCode::Internal,
                format!("the {} couldn't do {op}", kind.noun()),
            )
            .with_cause(why),
            SILICO_BUG,
        ),
    };
    let elements = failure.refusal.elements();
    // The one place in `edges` every edge at fault is at, if there's one.
    let places: std::collections::BTreeSet<usize> = elements
        .iter()
        .map(|&element| named(element).map(|(position, _)| position))
        .collect::<Option<_>>()
        .unwrap_or_default();
    let parameter = match places.first() {
        Some(position) if places.len() == 1 => Some(format!("edges.{position}")),
        Some(_) => Some("edges".to_owned()),
        None => failure.refusal.parameter().map(str::to_owned),
    };
    let error = error
        .with_elements(elements.into_iter().map(element_name))
        .with_elements_total(failure.refusal.elements_total());
    let error = match parameter {
        Some(parameter) => error.with_parameter(parameter),
        None => error,
    };
    let error = error.with_suggestion(described.suggestion.unwrap_or_else(|| fallback.to_owned()));
    match words {
        Some(failure) => error.with_worker_failure(op, failure),
        None => error,
    }
}

fn internal(kind: WorkerKind, what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("couldn't start the {}: {what} failed", kind.noun()),
    )
    .with_cause(e.to_string())
    .with_suggestion(SILICO_BUG)
}

/// A step of starting a worker failed in this process: short of descriptors, threads, or
/// memory, most likely.
fn start_failed(kind: WorkerKind, what: &str, e: &io::Error) -> Error {
    shortage(
        e,
        &format!("couldn't start the {}: {what} failed", kind.noun()),
    )
    .unwrap_or_else(|| internal(kind, what, e))
}

/// `message` as a `resource_exhausted` error, if `e` says this process, or the system, is
/// short of something: open files, processes or threads, memory, or buffers.
pub(crate) fn shortage(e: &io::Error, message: &str) -> Option<Error> {
    let advice = match e.raw_os_error()? {
        libc::EMFILE | libc::ENFILE => {
            "Silico has too many files open: close some documents or other programs, or raise \
             the limit on open files (ulimit -n)"
        }
        libc::EAGAIN | libc::ENOMEM | libc::ENOBUFS => {
            "the system is short of processes or memory: close some documents or other \
             programs, then try again"
        }
        _ => return None,
    };
    Some(
        Error::new(ErrorCode::ResourceExhausted, message)
            .with_cause(e.to_string())
            .with_suggestion(advice),
    )
}

/// A worker executable: the one `env` names, or `binary` next to the running executable.
/// Cargo puts test binaries in a `deps` directory below the worker's, so from there it looks
/// one directory up. `noun` names the worker in errors, and `build` says how to build it.
pub(crate) fn worker_path(
    env: &str,
    binary: &str,
    noun: &str,
    build: &str,
) -> Result<PathBuf, Error> {
    if let Some(value) = std::env::var_os(env) {
        // Absolute, so the file checked is the one run: a bare name would otherwise be
        // checked in the working directory and looked up on PATH.
        let path = std::path::absolute(&value).unwrap_or_else(|_| PathBuf::from(&value));
        if path.is_file() {
            return Ok(path);
        }
        return Err(Error::new(
            ErrorCode::WorkerUnavailable,
            format!(
                "{env} names {:?}, which isn't a file",
                value.to_string_lossy()
            ),
        )
        .with_suggestion(format!(
            "point {env} at the {binary} binary, or unset it to use the one beside Silico"
        )));
    }
    let exe = std::env::current_exe().map_err(|e| {
        Error::new(ErrorCode::Internal, "can't find the running executable")
            .with_cause(e.to_string())
    })?;
    let name = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
    let dir = exe.parent().unwrap_or(Path::new("."));
    let dir = if dir.file_name().is_some_and(|n| n == "deps") {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    let worker = dir.join(&name);
    if worker.is_file() {
        Ok(worker)
    } else {
        Err(Error::new(
            ErrorCode::WorkerUnavailable,
            format!("no {noun} at {}", worker.display()),
        )
        .with_suggestion(format!("build it ({build}), or name it in {env}")))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use silico_protocol::Failure;

    use super::{
        FAULTS, KillOnDrop, MAX_LOG_LINE, STDERR_TAIL_LINES, WorkerKind, crash_advice,
        describe_exit, failure_error, failure_error_with_edge_names, forward_logs, lock,
        signal_name,
    };
    use crate::ErrorCode;
    use crate::references::EdgeNames;
    use tracing_subscriber::layer::SubscriberExt;

    /// A directory of its own for one test, `silico-<name>-…`, removed when it's dropped,
    /// so a test that fails leaves nothing behind either.
    fn scratch(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("silico-{name}-"))
            .tempdir()
            .unwrap()
    }

    /// A worker script, `body` after its shebang, in `dir`.
    fn script(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.path().join("worker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn draining_reads_everything_a_worker_sent_up_to_where_its_replies_end() {
        // It says it started (an empty frame), then sends a frame that doesn't decode (one
        // byte, 0xff), which ends its reply thread, and runs on. Drained, everything it sent
        // has been read: that it started, and the end, with nothing left waiting.
        let dir = scratch("drain");
        let path = script(
            &dir,
            "printf '\\0\\0\\0\\0\\1\\0\\0\\0\\377' >&3\nexec sleep 600",
        );
        let mut worker = super::Worker::spawn(&path).unwrap();
        worker.drain_replies(std::time::Instant::now() + std::time::Duration::from_secs(10));
        assert!(worker.has_started());
        assert!(matches!(
            worker.replies.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn an_exit_is_waited_for_through_its_grace_and_none_said_only_after_it() {
        use std::time::{Duration, Instant};
        // One that runs on: none, and not before the grace is out.
        let dir = scratch("runs-on");
        let path = script(&dir, "exec sleep 600");
        let mut worker = super::Worker::spawn(&path).unwrap();
        let start = Instant::now();
        assert_eq!(worker.exit_status_within(Duration::from_millis(200)), None);
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "{:?}",
            start.elapsed()
        );
        // One that exits: how, as soon as it has.
        let dir = scratch("exits");
        let path = script(&dir, "exit 3");
        let mut worker = super::Worker::spawn(&path).unwrap();
        let status = worker.exit_status_within(Duration::from_secs(10));
        assert_eq!(status.and_then(|s| s.code()), Some(3), "{status:?}");
    }

    #[test]
    fn an_executables_stamp_is_its_files_and_is_renewed_when_a_change_keeps_its_bytes() {
        // Found by review: dropping the renewal, or the stamp's shortcut, only made every
        // check hash the file again, which no test could see. The stamp taken with the hash
        // is the file's (from its path); a change that keeps the bytes (a relink: the same
        // bytes as a new file, renamed over it, so another inode) is hashed, found the same,
        // and renews the stamp.
        let dir = scratch("stamp");
        let path = dir.path().join("worker");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        let exe = super::Executable::of(&path, WorkerKind::Kernel).unwrap();
        assert_eq!(Some(*lock(&exe.stamp)), super::Stamp::of(&path));
        let before = *lock(&exe.stamp);
        let beside = dir.path().join("worker.new");
        std::fs::write(&beside, b"#!/bin/sh\n").unwrap();
        std::fs::rename(&beside, &path).unwrap();
        let relinked = super::Stamp::of(&path);
        assert_ne!(
            relinked,
            Some(before),
            "a relink changes the stamp (its inode)"
        );
        exe.check().unwrap();
        assert_eq!(Some(*lock(&exe.stamp)), relinked);
    }

    #[test]
    fn only_a_kernel_workers_refusal_of_a_measure_keeps_its_failure_and_it_words_the_same() {
        // What the rebuild cache keeps of a refusal, so the core reading it words it: only a
        // kernel worker's refusal of a measure keeps the worker's failure, and wording that
        // again gives the same error.
        let failure = Failure {
            refusal: silico_protocol::Refusal::ResultInvalid {
                operation: silico_protocol::Operation::Other,
                edges: vec![u32::MAX],
            },
            cause: Some("OCCT's words".to_owned()),
        };
        // The measure's name is the protocol's for it.
        let summarize = silico_protocol::Op::Summarize {
            shape: silico_protocol::ShapeData::checked(
                silico_protocol::Blob::inline(Vec::new()),
                true,
            ),
        };
        assert_eq!(summarize.name(), super::MEASURE);
        let measure = failure_error("Summarize", failure.clone());
        let (op, kept) = measure.worker_failure().unwrap();
        assert_eq!((op, kept), ("Summarize", &failure));
        let again = failure_error(op, kept.clone());
        assert_eq!(again, measure);
        assert_eq!(again.elements(), ["Edge4294967295"]);
        // Another operation's, a named fillet's, and the sketch worker's keep none.
        assert!(
            failure_error("Fillet", failure.clone())
                .worker_failure()
                .is_none()
        );
        let names = EdgeNames::default();
        let named = failure_error_with_edge_names(
            WorkerKind::Kernel,
            "Summarize",
            failure.clone(),
            Some(&names),
        );
        assert!(named.worker_failure().is_none());
        let sketch = failure_error_with_edge_names(WorkerKind::Sketch, "Summarize", failure, None);
        assert!(sketch.worker_failure().is_none());
        // Keeping it is no part of what the error is: the same fields, built by hand, are
        // equal to it.
        let mut built = crate::Error::new(measure.code(), measure.message())
            .with_elements(measure.elements().iter().cloned())
            .with_elements_total(measure.elements_total());
        if let Some(cause) = measure.cause() {
            built = built.with_cause(cause);
        }
        if let Some(suggestion) = measure.suggestion() {
            built = built.with_suggestion(suggestion);
        }
        if let Some(parameter) = measure.parameter() {
            built = built.with_parameter(parameter);
        }
        assert!(built.worker_failure().is_none());
        assert_eq!(built, measure);
    }

    #[test]
    fn a_sketch_workers_crash_advice_names_it_and_its_solver() {
        use super::Reached::Request;
        use super::WorkerKind::Sketch;
        let signalled = |signal| Some(ExitStatus::from_raw(signal));
        assert_eq!(
            crash_advice(Sketch, signalled(libc::SIGSEGV), Request),
            "change the operation's values: these crashed the sketch solver, and the same \
             request would crash it again"
        );
        assert_eq!(
            crash_advice(Sketch, signalled(libc::SIGTERM), Request),
            "try the request again: the sketch worker was stopped from outside, stopped \
             responding, or was gone before the request reached it, not by these values"
        );
        assert_eq!(
            super::broken_worker(Sketch),
            "this is a bug in Silico's sketch worker, not in the request: report it (the \
             worker was stopped, and the next request starts another)"
        );
    }

    #[test]
    fn crash_advice_follows_how_the_worker_ended() {
        use super::Reached::{NotStarted, Request, Started};
        // From the signals' meanings, not from `FAULTS`: a fault of the worker's own once
        // the request reached it is the values'; before it started, the install's.
        let values = "change the operation's values";
        let again = "try the request again";
        let install = "check Silico's install";
        let signalled = |signal| Some(ExitStatus::from_raw(signal));
        for signal in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGABRT,
            libc::SIGTRAP,
            libc::SIGSYS,
        ] {
            for (reached, says) in [(Request, values), (Started, again), (NotStarted, install)] {
                let advice = crash_advice(super::WorkerKind::Kernel, signalled(signal), reached);
                assert!(advice.starts_with(says), "{signal} {reached:?}: {advice}");
            }
        }
        for (reached, says) in [
            (Request, "try the request once more"),
            (Started, again),
            (NotStarted, again),
        ] {
            let advice = crash_advice(super::WorkerKind::Kernel, signalled(libc::SIGKILL), reached);
            assert!(advice.starts_with(says), "{reached:?}: {advice}");
        }
        // Killed before it started: from outside, or, again and again, the system refusing
        // the binary.
        let unstarted = crash_advice(
            super::WorkerKind::Kernel,
            signalled(libc::SIGKILL),
            NotStarted,
        );
        assert!(unstarted.contains(install), "{unstarted}");
        assert!(
            !crash_advice(super::WorkerKind::Kernel, signalled(libc::SIGKILL), Started)
                .contains(install)
        );
        for reached in [Request, Started, NotStarted] {
            for signal in [
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGHUP,
                libc::SIGUSR1,
                libc::SIGPIPE,
            ] {
                let advice = crash_advice(super::WorkerKind::Kernel, signalled(signal), reached);
                assert!(advice.starts_with(again), "{signal} {reached:?}: {advice}");
            }
            // Killed after it stopped responding: no status.
            assert!(
                crash_advice(super::WorkerKind::Kernel, None, reached).starts_with(again),
                "{reached:?}"
            );
            // An exit code (3, in the status's high byte) is the install's, either way.
            let exited = crash_advice(
                super::WorkerKind::Kernel,
                Some(ExitStatus::from_raw(3 << 8)),
                reached,
            );
            assert!(exited.starts_with(install), "{reached:?}: {exited}");
        }
    }

    #[test]
    fn a_start_short_of_files_processes_or_memory_is_told_apart_from_a_bad_install() {
        use std::io;

        use super::shortage;
        let says = |errno| shortage(&io::Error::from_raw_os_error(errno), "couldn't start");
        for (errno, advice) in [
            (libc::EMFILE, "too many files open"),
            (libc::ENFILE, "too many files open"),
            (libc::EAGAIN, "processes or memory"),
            (libc::ENOMEM, "processes or memory"),
            (libc::ENOBUFS, "processes or memory"),
        ] {
            let error = says(errno).unwrap_or_else(|| panic!("{errno}: not a shortage"));
            assert_eq!(error.code(), ErrorCode::ResourceExhausted, "{error}");
            assert_eq!(error.message(), "couldn't start", "{error}");
            // The OS's own words for it.
            assert_eq!(
                error.cause(),
                Some(io::Error::from_raw_os_error(errno).to_string().as_str()),
                "{error}"
            );
            assert!(
                error.suggestion().is_some_and(|s| s.contains(advice)),
                "{error}"
            );
        }
        // The install's: missing, not executable, not a program, a path through a file.
        for errno in [libc::ENOENT, libc::EACCES, libc::ENOEXEC, libc::ENOTDIR] {
            assert_eq!(says(errno), None, "{errno}");
        }
        // Not the OS's at all (a path with a NUL byte in it): not a shortage.
        assert_eq!(
            shortage(
                &io::Error::other("nul byte found in provided data"),
                "couldn't start"
            ),
            None
        );
    }

    #[test]
    fn every_fault_signal_is_named() {
        for signal in FAULTS {
            assert!(signal_name(signal).is_some(), "signal {signal}");
        }
    }

    #[test]
    fn wait_for_sleeps_until_its_timeout_and_takes_any_timeout() {
        use std::os::unix::net::UnixStream;
        use std::time::{Duration, Instant};
        // Nothing arrives, so it sleeps the whole timeout.
        let (a, _b) = UnixStream::pair().unwrap();
        let start = Instant::now();
        super::wait_for(
            &a,
            rustix::event::PollFlags::IN,
            Some(Duration::from_millis(200)),
        )
        .unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(190),
            "{:?}",
            start.elapsed()
        );
        // Any timeout, however long, is accepted (the socket is writable, so it returns).
        for days in [1, 24, 25, 30, 365 * 100] {
            super::wait_for(
                &a,
                rustix::event::PollFlags::OUT,
                Some(Duration::from_secs(days * 86_400)),
            )
            .unwrap();
        }
        super::wait_for(&a, rustix::event::PollFlags::OUT, Some(Duration::MAX)).unwrap();
    }

    #[test]
    fn a_refusal_must_fit_the_request_it_answers() {
        use silico_protocol::{DressingKind, EdgeTrouble, Operation, Refusal, Trouble};

        use super::refusal_fits;
        let fillet = DressingKind::Fillet;
        let refused = |edges: &[u32]| Refusal::EdgesRefused {
            dressing: fillet,
            troubles: edges
                .iter()
                .map(|&edge| EdgeTrouble {
                    edge,
                    trouble: Trouble::Smooth,
                })
                .collect(),
        };
        let occt = |edges: Vec<u32>| Refusal::Occt {
            operation: Operation::Dressing { dressing: fillet },
            edges,
        };
        let fits = |refusal: &Refusal, listed: Option<&[u32]>| refusal_fits(refusal, listed);
        // Edges the request lists, each once, in order.
        assert_eq!(fits(&refused(&[3, 5]), Some(&[5, 3, 9])), Ok(()));
        assert_eq!(fits(&occt(vec![2]), Some(&[2])), Ok(()));
        assert_eq!(
            fits(
                &Refusal::EdgeMissing {
                    edge: 13,
                    count: 12
                },
                Some(&[13])
            ),
            Ok(())
        );
        assert_eq!(fits(&Refusal::EdgeTwice { edge: 1 }, Some(&[1, 1])), Ok(()));
        assert_eq!(
            fits(&Refusal::NoEdges { dressing: fillet }, Some(&[])),
            Ok(())
        );
        // Only the core refuses a mesh for its unmeshed faces.
        let unmeshed = Refusal::FacesUnmeshed {
            faces: vec![1, 4],
            count: 2,
        };
        assert!(fits(&unmeshed, None).is_err());
        assert_eq!(fits(&Refusal::BrepCorrupted, None), Ok(()));
        // Not: a forged refusal of a million edges answering a request for one costs
        // nothing to refuse.
        let forged = Refusal::EdgesRefused {
            dressing: fillet,
            troubles: vec![
                EdgeTrouble {
                    edge: 1,
                    trouble: Trouble::Smooth,
                };
                1 << 20
            ],
        };
        assert!(fits(&forged, Some(&[1])).is_err());
        let counting: Vec<u32> = (1..=1 << 20).collect();
        assert!(fits(&refused(&counting), Some(&[1])).is_err());
        assert!(fits(&refused(&[]), Some(&[1])).is_err());
        assert!(fits(&refused(&[5, 3]), Some(&[3, 5])).is_err());
        assert!(fits(&refused(&[4]), Some(&[3, 5])).is_err());
        assert!(fits(&refused(&[3]), None).is_err());
        assert!(fits(&occt(vec![7]), Some(&[1])).is_err());
        assert!(fits(&Refusal::EdgeMissing { edge: 0, count: 12 }, Some(&[1])).is_err());
        assert!(fits(&Refusal::NoEdges { dressing: fillet }, Some(&[1])).is_err());
        let invalid = Refusal::ResultInvalid {
            operation: Operation::Dressing { dressing: fillet },
            edges: vec![7],
        };
        assert!(fits(&invalid, Some(&[1])).is_err());
        assert!(fits(&Refusal::EdgeTwice { edge: 9 }, Some(&[1, 1])).is_err());
        // The edges OCCT couldn't fillet: some, each listed, once, in order.
        let failed = |edges: Vec<u32>| Refusal::EdgesFailed {
            dressing: fillet,
            edges,
        };
        assert_eq!(fits(&failed(vec![2, 10]), Some(&[10, 1, 2])), Ok(()));
        assert!(fits(&failed(vec![]), Some(&[1])).is_err());
        assert!(fits(&failed(vec![10, 2]), Some(&[2, 10])).is_err());
        assert!(fits(&failed(vec![2, 2]), Some(&[2])).is_err());
        assert!(fits(&failed(vec![3]), Some(&[2])).is_err());
        assert!(fits(&failed(vec![2]), None).is_err());
    }

    #[test]
    fn a_documents_edges_are_named_by_their_places_and_names_not_their_numbers() {
        use silico_protocol::{DressingKind, Refusal};
        let mut names = EdgeNames::default();
        names.insert(3, 0, "k7f2qx3m.box.Edge1");
        names.insert(7, 2, "k7f2qx3m.box.Edge5");
        // Named twice: its first name, and first place, stand.
        names.insert(7, 4, "k7f2qx3m.box.Edge9");
        let failed = |edges: Vec<u32>| Failure {
            refusal: Refusal::EdgesFailed {
                dressing: DressingKind::Fillet,
                edges,
            },
            cause: None,
        };
        let e = failure_error_with_edge_names(
            WorkerKind::Kernel,
            "Fillet",
            failed(vec![3, 7]),
            Some(&names),
        );
        assert_eq!(
            (e.code(), e.elements(), e.parameter()),
            (
                ErrorCode::KernelFailed,
                &[
                    "k7f2qx3m.box.Edge1".to_owned(),
                    "k7f2qx3m.box.Edge5".to_owned()
                ][..],
                Some("edges")
            )
        );
        assert_eq!(e.cause(), Some("OCCT couldn't fillet edges.0 and edges.2"));
        // One edge at fault: the parameter is its place.
        let e = failure_error_with_edge_names(
            WorkerKind::Kernel,
            "Fillet",
            failed(vec![7]),
            Some(&names),
        );
        assert_eq!(
            (e.elements(), e.parameter()),
            (&["k7f2qx3m.box.Edge5".to_owned()][..], Some("edges.2"))
        );
        // An edge the document doesn't name (no request lists one, but a refusal is only
        // checked to fit the request) keeps its number, and the parameter is the refusal's.
        let e = failure_error_with_edge_names(
            WorkerKind::Kernel,
            "Fillet",
            failed(vec![3, 9]),
            Some(&names),
        );
        assert_eq!(
            (e.elements(), e.parameter()),
            (
                &["k7f2qx3m.box.Edge1".to_owned(), "Edge9".to_owned()][..],
                None
            )
        );
        assert_eq!(e.cause(), Some("OCCT couldn't fillet edges.0 and Edge9"));
        // Without names, numbers, as before a document's names.
        let e =
            failure_error_with_edge_names(WorkerKind::Kernel, "Fillet", failed(vec![3, 7]), None);
        assert_eq!(
            (e.elements(), e.parameter()),
            (&["Edge3".to_owned(), "Edge7".to_owned()][..], None)
        );
        // A long name is cut short among the elements, as a broken reference's is; the
        // words say only its place.
        let long = format!(
            "k7f2qx3m.fillet.Edge({})",
            "G a91cw0de.cylinder.Face1, ".repeat(20)
        );
        let mut names = EdgeNames::default();
        names.insert(3, 1, &long);
        let e = failure_error_with_edge_names(
            WorkerKind::Kernel,
            "Fillet",
            failed(vec![3]),
            Some(&names),
        );
        assert_eq!(e.elements(), &[crate::references::cut_short(&long)][..]);
        assert!(e.elements()[0].chars().count() < long.chars().count());
        assert_eq!(e.cause(), Some("OCCT couldn't fillet edges.1"));
    }

    #[test]
    fn running_out_of_memory_is_a_shortage_not_the_values_fault() {
        use silico_protocol::Refusal;
        let e = failure_error(
            "Fuse",
            Failure {
                refusal: Refusal::OutOfMemory,
                cause: Some("Standard_OutOfMemory".to_owned()),
            },
        );
        assert_eq!(
            (e.code(), e.message(), e.cause(), e.suggestion()),
            (
                ErrorCode::ResourceExhausted,
                "Fuse ran out of memory",
                Some("Standard_OutOfMemory"),
                Some("free some memory, or make the part or sketch simpler, and try again")
            )
        );
    }

    #[test]
    fn kernel_failures_get_their_own_code_and_always_a_suggestion() {
        use silico_protocol::{DressingKind, EdgeTrouble, Operation, Refusal, Trouble};
        let occt = |operation, edges| Failure {
            refusal: Refusal::Occt { operation, edges },
            cause: Some("StdFail_NotDone".to_owned()),
        };
        // No suggestion of its own: the code's.
        let e = failure_error("Fillet", occt(Operation::Other, Vec::new()));
        assert_eq!(
            (e.code(), e.message(), e.cause(), e.suggestion()),
            (
                ErrorCode::KernelFailed,
                "Fillet failed",
                Some("StdFail_NotDone"),
                Some("change the operation's values: the kernel can't build it from these")
            )
        );
        assert!(e.elements().is_empty(), "{e}");
        let e = failure_error(
            "Fillet",
            Failure::new(Refusal::ResultInvalid {
                operation: Operation::Other,
                edges: Vec::new(),
            }),
        );
        assert_eq!(
            (e.code(), e.message(), e.cause(), e.suggestion()),
            (
                ErrorCode::InvalidResult,
                "Fillet gave an invalid result, which was refused",
                Some("the operation's result isn't a valid solid"),
                Some(
                    "change the operation's values: the kernel can't build a valid result from these"
                )
            )
        );
        // Its own suggestion, and the edge at fault, named by the core.
        let fillet = Operation::Dressing {
            dressing: DressingKind::Fillet,
        };
        let e = failure_error("Fillet", occt(fillet, vec![1]));
        assert_eq!(e.elements(), ["Edge1"], "{e}");
        assert!(
            e.suggestion()
                .is_some_and(|s| s.starts_with("if the fillet runs past a face beside the edge")),
            "{e}"
        );
        // A refusal of edges 3 and 5 names both.
        let e = failure_error(
            "Fillet",
            Failure::new(Refusal::EdgesRefused {
                dressing: DressingKind::Fillet,
                troubles: vec![
                    EdgeTrouble {
                        edge: 3,
                        trouble: Trouble::Smooth,
                    },
                    EdgeTrouble {
                        edge: 5,
                        trouble: Trouble::OneFace,
                    },
                ],
            }),
        );
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
        assert_eq!(e.elements(), ["Edge3", "Edge5"], "{e}");
        assert!(e.message().starts_with("Edge3 joins faces"), "{e}");
        assert!(
            e.message().ends_with("; OCCT won't fillet Edge5 either"),
            "{e}"
        );
        assert_eq!(
            e.suggestion(),
            Some("leave Edge3 and Edge5 out of the fillet")
        );
        // A value refused, named as data.
        let e = failure_error(
            "Fillet",
            Failure::new(Refusal::InvalidParameter {
                parameter: "radius".to_owned(),
                message: "the fillet's radius must be a length above 1e-7 mm".to_owned(),
            }),
        );
        assert_eq!(e.parameter(), Some("radius"), "{e}");
        assert_eq!(
            e.message(),
            "the fillet's radius must be a length above 1e-7 mm"
        );
        assert_eq!(e.suggestion(), Some("change the value the message names"));
        // A test operation on a production worker: Silico's bug.
        let e = failure_error(
            "DebugSleep",
            Failure::new(Refusal::TestOperation {
                op: "DebugSleep".to_owned(),
            }),
        );
        assert_eq!(e.code(), ErrorCode::Internal, "{e}");
        assert_eq!(
            e.cause(),
            Some("DebugSleep is a test operation, not answered by this worker")
        );
        assert_eq!(e.suggestion(), Some(super::SILICO_BUG));
    }

    #[test]
    fn a_refusal_of_more_faces_than_it_names_counts_them_all() {
        let refusal = silico_protocol::Refusal::FacesUnmeshed {
            faces: (1..=100).collect(),
            count: 250,
        };
        let e = failure_error("Tessellate", Failure::new(refusal));
        assert_eq!(e.elements().len(), 100, "{e}");
        assert_eq!(e.elements()[99], "Face100", "{e}");
        assert_eq!(e.elements_total(), 250, "{e}");
        assert!(e.to_string().contains("Face100, and 150 more"), "{e}");
    }

    #[test]
    fn exits_and_signals_are_named_by_this_platforms_numbers() {
        // A raw wait status: a signal number in the low 7 bits, or an exit code shifted up.
        let named = |signal: i32, name: &str| format!("it was killed by signal {signal} ({name})");
        let cases = [
            (0, "it exited with code 0".to_owned()),
            (3 << 8, "it exited with code 3".to_owned()),
            (libc::SIGILL, named(libc::SIGILL, "SIGILL")),
            (libc::SIGABRT, named(libc::SIGABRT, "SIGABRT")),
            (libc::SIGBUS, named(libc::SIGBUS, "SIGBUS")),
            (libc::SIGFPE, named(libc::SIGFPE, "SIGFPE")),
            (libc::SIGKILL, named(libc::SIGKILL, "SIGKILL")),
            (libc::SIGSEGV, named(libc::SIGSEGV, "SIGSEGV")),
            (libc::SIGTERM, named(libc::SIGTERM, "SIGTERM")),
            (
                libc::SIGUSR1,
                format!("it was killed by signal {}", libc::SIGUSR1),
            ),
        ];
        for (raw, want) in cases {
            assert_eq!(
                describe_exit(ExitStatus::from_raw(raw)),
                want,
                "raw status {raw}"
            );
        }
    }

    #[test]
    fn signal_numbers_that_differ_between_platforms_arent_misnamed() {
        // SIGBUS is 7 on Linux and 10 on macOS; the other number is something else.
        let other = if libc::SIGBUS == 7 { 10 } else { 7 };
        assert!(!describe_exit(ExitStatus::from_raw(other)).contains("SIGBUS"));
    }

    fn logs_of(bytes: &[u8]) -> Vec<String> {
        let tail = Mutex::new(VecDeque::new());
        let (queue, _queued) = private_queue(super::LOG_QUEUE_BYTES);
        forward_logs(bytes, &tail, &Mutex::new(super::Callers::default()), &queue);
        tail.into_inner().unwrap().into_iter().collect()
    }

    /// A queue to a sink thread of the test's own, holding at most `most` bytes: the test
    /// emits what it takes off the receiver, as the sink thread does.
    fn private_queue(most: usize) -> (super::LogQueue, std::sync::mpsc::Receiver<super::Queued>) {
        let (items, queued) = std::sync::mpsc::channel();
        let queue = super::LogQueue::new(1, items, std::sync::Arc::default(), most);
        (queue, queued)
    }

    /// Reads `line` and emits it at once, as the sink thread does, giving its summary.
    fn emit_worker_log(line: &str, callers: &Mutex<super::Callers>) -> Option<String> {
        match super::read_worker_log(line, callers) {
            super::ReadLine::Line(read, summary) => {
                read.emit(1);
                Some(summary)
            }
            super::ReadLine::Done(_) => None,
        }
    }

    type Seen = std::sync::Arc<Mutex<Vec<(tracing::Level, String)>>>;

    /// Records each event's level and message.
    struct Events(Seen);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Events {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Message(String);
            impl tracing::field::Visit for Message {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), message.0));
        }
    }

    fn dropped(n: u64) -> (tracing::Level, String) {
        (
            tracing::Level::WARN,
            format!("{n} of the worker's log lines were dropped: the log sink fell behind"),
        )
    }

    fn warn(message: &str) -> (tracing::Level, String) {
        (tracing::Level::WARN, message.to_owned())
    }

    #[test]
    fn a_worker_line_goes_under_the_span_its_request_was_asked_in_at_every_level() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
        use tracing_subscriber::registry::LookupSpan;
        type Seen = Arc<Mutex<Vec<Vec<&'static str>>>>;
        struct Events(Seen);
        impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Events {
            fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
                let scope = ctx
                    .event_scope(event)
                    .map(|spans| spans.map(|span| span.name()).collect())
                    .unwrap_or_default();
                self.0.lock().unwrap().push(scope);
            }
        }
        for level in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] {
            let seen: Seen = Arc::default();
            let subscriber = tracing_subscriber::registry().with(
                Events(Arc::clone(&seen)).with_filter(tracing::level_filters::LevelFilter::TRACE),
            );
            tracing::subscriber::with_default(subscriber, || {
                let mut callers = super::Callers::default();
                callers.push(7, tracing::error_span!("first_step"));
                callers.push(8, tracing::error_span!("second_step"));
                let callers = Mutex::new(callers);
                let line = |trace: u64| {
                    format!(
                        "{{\"level\":\"{level}\",\"fields\":{{\"message\":\"OCCT said\"}},\"span\":{{\"trace\":{trace}}}}}"
                    )
                };
                // The second request was asked before the first's line was read: each line
                // still goes under its own request's span.
                emit_worker_log(&line(7), &callers);
                emit_worker_log(&line(8), &callers);
                // A line of no request goes under none.
                emit_worker_log(
                    &format!("{{\"level\":\"{level}\",\"fields\":{{\"message\":\"no request\"}}}}"),
                    &callers,
                );
                // The done line lets the first request's span go, and isn't a log line.
                assert_eq!(
                    emit_worker_log("{\"silico_request_done\":7}", &callers),
                    None
                );
                emit_worker_log(&line(7), &callers);
                assert_eq!(callers.lock().unwrap().0.len(), 1);
            });
            let seen = seen.lock().unwrap().clone();
            assert_eq!(
                seen,
                [vec!["first_step"], vec!["second_step"], vec![], vec![]],
                "{level}"
            );
        }
    }

    #[test]
    fn a_done_line_lets_older_unanswered_requests_go_too() {
        // A request refused before it was sent never gets a done line of its own.
        let mut callers = super::Callers::default();
        callers.push(7, tracing::Span::none());
        callers.push(8, tracing::Span::none());
        callers.push(9, tracing::Span::none());
        callers.done(8);
        assert_eq!(
            callers
                .0
                .iter()
                .map(|(trace, _)| *trace)
                .collect::<Vec<_>>(),
            [9]
        );
    }

    #[test]
    fn done_lines_stay_out_of_the_crash_tail() {
        let lines = logs_of(b"{\"silico_request_done\":7}\nLAST WORDS\n");
        assert_eq!(lines, ["LAST WORDS"]);
    }

    #[test]
    fn the_pipe_holds_fewer_done_lines_than_the_core_remembers() {
        // What a worker can write before the log thread reads any, on this platform: a
        // non-blocking pipe filled until the system says it's full. A write this short is
        // atomic (at most PIPE_BUF), so each goes in whole or not at all.
        use std::io::Write;
        let (reader, mut writer) = std::io::pipe().unwrap();
        rustix::io::ioctl_fionbio(&writer, true).unwrap();
        // The shortest done line a worker writes (the first trace), so the most fit.
        let line = format!("{{\"{}\":1}}\n", silico_protocol::REQUEST_DONE);
        let mut held = 0usize;
        loop {
            match writer.write(line.as_bytes()) {
                Ok(n) => {
                    assert_eq!(n, line.len(), "a short write to a pipe is whole");
                    held += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert!(
            held < super::Callers::MOST,
            "the pipe held {held} done lines"
        );
        // No supported pipe holds less than Linux's least, two 4 KiB pages
        // (PIPE_MIN_DEF_BUFFERS), which is all a new pipe gets once its user's pipes pass
        // fs.pipe-user-pages-soft (as on a CI machine running several runners as one user).
        // Linux puts a short write on one page whole, so each page holds whole lines.
        assert!(held >= 2 * (4096 / line.len()), "the pipe held only {held}");
        drop(reader);
    }

    #[test]
    fn a_worker_that_never_says_a_request_is_done_holds_no_more_than_the_bound() {
        // Above the pipe's hold of done lines on the platforms Silico is checked on.
        assert_eq!(super::Callers::MOST, 4096);
        let mut callers = super::Callers::default();
        let most = super::Callers::MOST as u64;
        // One span pushed out (for the caller to warn of and let go) for each past the
        // bound, none before.
        let pushed_out = (0..most + 10)
            .filter(|&trace| callers.push(trace, tracing::Span::none()).is_some())
            .count();
        assert_eq!(pushed_out, 10);
        assert_eq!(callers.0.len(), super::Callers::MOST);
        assert_eq!(callers.0.front().map(|(trace, _)| *trace), Some(10));
        // A request never sent is let go at once, and only it.
        assert!(callers.forget(most + 5).is_some());
        assert!(callers.forget(most + 5).is_none());
        assert_eq!(callers.0.len(), super::Callers::MOST - 1);
        assert!(callers.0.iter().all(|(trace, _)| *trace != most + 5));
    }

    #[test]
    fn worker_log_lines_keep_their_level() {
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for level in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] {
            let captured = Captured::default();
            let writer = captured.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            let line = format!("{{\"level\":\"{level}\",\"fields\":{{\"message\":\"hello\"}}}}");
            tracing::subscriber::with_default(subscriber, || {
                emit_worker_log(&line, &Mutex::new(super::Callers::default()))
            });
            let out = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
            assert!(
                out.contains(&format!("{level:>5} silico::worker")),
                "{level}: {out}"
            );
        }
    }

    #[test]
    fn json_log_lines_are_summarized_by_level_and_message() {
        let lines = logs_of(
            b"{\"level\":\"WARN\",\"fields\":{\"message\":\"careful\"},\"span\":{\"trace\":7}}\n\
              not json at all\n",
        );
        assert_eq!(lines, ["WARN careful", "not json at all"]);
    }

    /// Gives `lines` one read at a time, and before line `drain_before` has the sink take
    /// one item, as a sink catching up does.
    struct Paced<'a> {
        lines: VecDeque<&'static [u8]>,
        read: usize,
        drain_before: usize,
        queued: &'a std::sync::mpsc::Receiver<super::Queued>,
        held: &'a std::sync::atomic::AtomicUsize,
    }

    impl std::io::Read for Paced<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.read == self.drain_before
                && let Ok(item) = self.queued.try_recv()
            {
                super::sink_one(item, self.held);
            }
            let Some(line) = self.lines.pop_front() else {
                return Ok(0);
            };
            self.read += 1;
            buf[..line.len()].copy_from_slice(line);
            Ok(line.len())
        }
    }

    #[test]
    fn a_stalled_sink_has_lines_dropped_and_counted_and_the_tail_kept_whole() {
        // Room for one short line: "one" waits, "two" and "three" are dropped; the sink
        // takes "one", and "four" has room; "five" and "six" are dropped. The count is said
        // where the first was dropped, once the sink gets there: all four, each once.
        let (queue, queued) = private_queue(3 + super::LOG_ITEM_COST + 3);
        let held = std::sync::Arc::clone(&queue.held);
        let lines: [&'static [u8]; 6] = [
            b"one\n", b"two\n", b"three\n", b"four\n", b"five\n", b"six\n",
        ];
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry().with(Events(seen.clone()));
        let tail = Mutex::new(VecDeque::new());
        tracing::subscriber::with_default(subscriber, || {
            let mut paced = Paced {
                lines: lines.into_iter().collect(),
                read: 0,
                drain_before: 3,
                queued: &queued,
                held: &held,
            };
            let callers = Mutex::new(super::Callers::default());
            forward_logs(&mut paced, &tail, &callers, &queue);
            drop(queue);
            for item in queued.iter() {
                super::sink_one(item, &held);
            }
        });
        assert_eq!(
            *seen.lock().unwrap(),
            [warn("one"), dropped(4), warn("four")]
        );
        // The crash report's tail has every line, dropped or not.
        assert_eq!(
            tail.into_inner().unwrap(),
            ["one", "two", "three", "four", "five", "six"]
        );
    }

    #[test]
    fn lines_dropped_are_counted_once_the_sink_catches_up_though_the_worker_says_no_more() {
        let (queue, queued) = private_queue(5 + super::LOG_ITEM_COST);
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry().with(Events(seen.clone()));
        let callers = Mutex::new(super::Callers::default());
        let tail = Mutex::new(VecDeque::new());
        forward_logs(&b"first\nsecond\nthird\n"[..], &tail, &callers, &queue);
        // The worker goes quiet, alive: the queue isn't closed, and the sink takes what's
        // there.
        tracing::subscriber::with_default(subscriber, || {
            while let Ok(item) = queued.try_recv() {
                super::sink_one(item, &queue.held);
            }
        });
        assert_eq!(*seen.lock().unwrap(), [warn("first"), dropped(2)]);
        // Said, the count starts again, and the queue's room is back.
        assert_eq!(queue.dropped.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(queue.held.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_requests_span_is_let_go_by_the_sink_thread_never_the_log_thread() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing_subscriber::layer::{Context, Layer};
        // Closing a span runs the sink (a span exporter, say), which may block.
        struct Closes(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> Layer<S> for Closes {
            fn on_close(&self, _: tracing::Id, _: Context<'_, S>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let closes = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(Closes(Arc::clone(&closes)));
        tracing::subscriber::with_default(subscriber, || {
            let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
            let callers = Mutex::new(super::Callers::default());
            // The caller's span, its only handle now held for the worker's lines.
            assert!(
                lock(&callers)
                    .push(7, tracing::error_span!("call"))
                    .is_none()
            );
            let tail = Mutex::new(VecDeque::new());
            forward_logs(
                &b"{\"silico_request_done\":7}\n"[..],
                &tail,
                &callers,
                &queue,
            );
            assert!(lock(&callers).0.is_empty());
            assert_eq!(closes.load(Ordering::SeqCst), 0, "closed on the log thread");
            for item in queued.try_iter() {
                super::sink_one(item, &queue.held);
            }
            assert_eq!(closes.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn a_requests_span_is_let_go_by_the_sink_thread_though_the_queue_is_full() {
        use std::sync::atomic::Ordering;
        // No room for any line: a done line's span still goes to the sink thread, counted,
        // so lines give way to it, and the log thread never closes it.
        let (queue, queued) = private_queue(0);
        let callers = Mutex::new(super::Callers::default());
        let tail = Mutex::new(VecDeque::new());
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || {
            assert!(
                lock(&callers)
                    .push(7, tracing::error_span!("call"))
                    .is_none()
            );
            forward_logs(
                &b"a line with no room\n{\"silico_request_done\":7}\n"[..],
                &tail,
                &callers,
                &queue,
            );
        });
        let items: Vec<_> = queued.try_iter().collect();
        let kinds: Vec<_> = items
            .iter()
            .map(|item| match &item.item {
                super::LogItem::Line(_) => "line",
                super::LogItem::Dropped(_) => "dropped",
                super::LogItem::Release(spans) if spans.len() == 1 => "release",
                super::LogItem::Release(_) => "release of other than one",
                super::LogItem::Flushed(_) => "flushed",
            })
            .collect();
        assert_eq!(kinds, ["dropped", "release"]);
        assert_eq!(queue.held.load(Ordering::SeqCst), super::LOG_ITEM_COST);
        for item in items {
            super::sink_one(item, &queue.held);
        }
        assert_eq!(queue.held.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_flush_is_said_once_everything_queued_before_it_is_emitted() {
        let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
        let tail = Mutex::new(VecDeque::new());
        forward_logs(
            &b"last\n"[..],
            &tail,
            &Mutex::new(super::Callers::default()),
            &queue,
        );
        let (emitted, flushed) = std::sync::mpsc::channel::<()>();
        queue.flushed(emitted);
        let line = queued.try_recv().unwrap();
        // Not yet: the line before it is still to be emitted.
        assert_eq!(
            flushed.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        super::sink_one(line, &queue.held);
        super::sink_one(queued.try_recv().unwrap(), &queue.held);
        assert_eq!(
            flushed.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }

    #[test]
    fn a_line_a_log_layer_panics_on_still_gives_its_bytes_back() {
        use std::sync::atomic::Ordering;
        use tracing_subscriber::layer::{Context, Layer};
        struct Panics;
        impl<S: tracing::Subscriber> Layer<S> for Panics {
            fn on_event(&self, _: &tracing::Event<'_>, _: Context<'_, S>) {
                panic!("a log layer with a bug");
            }
        }
        let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
        let tail = Mutex::new(VecDeque::new());
        forward_logs(
            &b"doomed\n"[..],
            &tail,
            &Mutex::new(super::Callers::default()),
            &queue,
        );
        assert_eq!(
            queue.held.load(Ordering::SeqCst),
            "doomed".len() + super::LOG_ITEM_COST
        );
        let subscriber = tracing_subscriber::registry().with(Panics);
        tracing::subscriber::with_default(subscriber, || {
            super::sink_one(queued.try_recv().unwrap(), &queue.held);
        });
        assert_eq!(queue.held.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_span_the_log_sink_doesnt_record_isnt_queued() {
        // It closes running nothing: at the default level, most requests' spans.
        let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
        let callers = Mutex::new(super::Callers::default());
        assert!(lock(&callers).push(7, tracing::Span::none()).is_none());
        let tail = Mutex::new(VecDeque::new());
        forward_logs(
            &b"{\"silico_request_done\":7}\n"[..],
            &tail,
            &callers,
            &queue,
        );
        assert!(lock(&callers).0.is_empty());
        assert_eq!(queued.try_iter().count(), 0);
    }

    #[test]
    fn spans_a_log_layer_panics_closing_are_each_let_go_without_aborting() {
        use std::sync::atomic::Ordering;
        use tracing_subscriber::layer::{Context, Layer};
        // Two let go at once (a done line after a request with none): a panic closing the
        // first, unwinding, mustn't close the second, which would panic again and abort.
        struct PanicsClosing;
        impl<S: tracing::Subscriber> Layer<S> for PanicsClosing {
            fn on_close(&self, _: tracing::Id, _: Context<'_, S>) {
                panic!("a log layer with a bug");
            }
        }
        let subscriber = tracing_subscriber::registry().with(PanicsClosing);
        tracing::subscriber::with_default(subscriber, || {
            let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
            let callers = Mutex::new(super::Callers::default());
            assert!(
                lock(&callers)
                    .push(1, tracing::error_span!("first"))
                    .is_none()
            );
            assert!(
                lock(&callers)
                    .push(2, tracing::error_span!("second"))
                    .is_none()
            );
            let tail = Mutex::new(VecDeque::new());
            forward_logs(
                &b"{\"silico_request_done\":2}\n"[..],
                &tail,
                &callers,
                &queue,
            );
            let release = queued.try_recv().unwrap();
            assert_eq!(queue.held.load(Ordering::SeqCst), 2 * super::LOG_ITEM_COST);
            super::sink_one(release, &queue.held);
            assert_eq!(queue.held.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn the_queue_holds_4_mib_of_lines_and_no_more() {
        // Lines of 100 bytes, each costing its message, the item's own size, and 64: the
        // queue takes as many as fit in 4 MiB, and drops the rest.
        assert_eq!(super::LOG_QUEUE_BYTES, 4 * 1024 * 1024);
        assert_eq!(
            super::LOG_ITEM_COST,
            std::mem::size_of::<super::Queued>() + 64
        );
        let fit = super::LOG_QUEUE_BYTES / (100 + super::LOG_ITEM_COST);
        let line = format!("{}\n", "x".repeat(100));
        let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
        let tail = Mutex::new(VecDeque::new());
        forward_logs(
            line.repeat(fit + 10).as_bytes(),
            &tail,
            &Mutex::new(super::Callers::default()),
            &queue,
        );
        let items: Vec<_> = queued.try_iter().collect();
        let lines = items
            .iter()
            .filter(|item| matches!(item.item, super::LogItem::Line(_)))
            .count();
        assert_eq!(lines, fit);
        assert_eq!(items.len(), fit + 1, "the lines, and one mark");
        assert_eq!(queue.dropped.load(std::sync::atomic::Ordering::SeqCst), 10);
    }

    #[test]
    fn with_the_sink_thread_gone_nothing_is_held_for_it() {
        use std::sync::atomic::Ordering;
        let (queue, queued) = private_queue(super::LOG_QUEUE_BYTES);
        drop(queued);
        let callers = Mutex::new(super::Callers::default());
        let tail = Mutex::new(VecDeque::new());
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || {
            assert!(
                lock(&callers)
                    .push(7, tracing::error_span!("call"))
                    .is_none()
            );
            forward_logs(
                &b"a line\n{\"silico_request_done\":7}\n"[..],
                &tail,
                &callers,
                &queue,
            );
        });
        assert_eq!(queue.held.load(Ordering::SeqCst), 0);
        assert_eq!(queue.dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_span_pushed_out_of_the_callers_is_warned_of() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing_subscriber::layer::{Context, Layer};
        struct Warnings(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> Layer<S> for Warnings {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                if *event.metadata().level() == tracing::Level::WARN {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        let warnings = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(Arc::clone(&warnings)));
        tracing::subscriber::with_default(subscriber, || {
            super::pushed_out_of_callers(None);
            assert_eq!(warnings.load(Ordering::SeqCst), 0);
            super::pushed_out_of_callers(Some(tracing::Span::none()));
            assert_eq!(warnings.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn text_that_isnt_utf8_doesnt_stop_the_reading() {
        let lines = logs_of(b"before\n\xff\xfe\nLAST WORDS\n");
        assert_eq!(lines, ["before", "\u{fffd}\u{fffd}", "LAST WORDS"]);
    }

    #[test]
    fn a_long_line_is_cut_to_one_line() {
        let mut bytes = vec![b'x'; MAX_LOG_LINE * 2 + 10];
        bytes.extend(b"\nnext\n");
        let lines = logs_of(&bytes);
        assert_eq!(
            lines.len(),
            2,
            "{:?}",
            lines.iter().map(String::len).collect::<Vec<_>>()
        );
        assert_eq!(lines[0], "x".repeat(MAX_LOG_LINE));
        assert_eq!(lines[1], "next");
        // A line of exactly the maximum, newline included, is whole.
        let mut bytes = vec![b'y'; MAX_LOG_LINE - 1];
        bytes.extend(b"\nnext\n");
        assert_eq!(
            logs_of(&bytes),
            ["y".repeat(MAX_LOG_LINE - 1), "next".to_owned()]
        );
    }

    #[test]
    fn long_lines_are_cut_and_the_tail_is_bounded() {
        let mut bytes = vec![b'x'; MAX_LOG_LINE * 2 + 10];
        bytes.push(b'\n');
        for i in 0..STDERR_TAIL_LINES + 5 {
            bytes.extend(format!("line {i}\n").bytes());
        }
        let lines = logs_of(&bytes);
        assert_eq!(lines.len(), STDERR_TAIL_LINES);
        assert_eq!(
            lines.last().unwrap(),
            &format!("line {}", STDERR_TAIL_LINES + 4)
        );
        assert!(lines.iter().all(|l| l.len() <= MAX_LOG_LINE));
    }

    #[test]
    fn a_kill_on_drop_guard_kills_and_reaps_its_process() {
        // The guard `Worker::spawn` holds until a worker is set up: an early return must not
        // leave the process running.
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id().to_string();
        // Killed, not only waited for: `sleep` would end on its own after 30 s.
        let start = std::time::Instant::now();
        drop(KillOnDrop(Some(child)));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        // Gone and reaped: `kill -0` finds no such process.
        let found = std::process::Command::new("kill")
            .args(["-0", &pid])
            .status()
            .unwrap();
        assert!(!found.success(), "process {pid} outlived its guard");
    }

    #[test]
    fn a_log_line_of_up_to_8_kib_is_kept_whole() {
        // The limit is 8 KiB, newline included, written out here rather than taken from
        // MAX_LOG_LINE, so a smaller limit fails.
        let mut bytes = vec![b'y'; 8 * 1024 - 1];
        bytes.extend(b"\nnext\n");
        assert_eq!(
            logs_of(&bytes),
            ["y".repeat(8 * 1024 - 1), "next".to_owned()]
        );
        // One byte more is cut to 8 KiB, and the rest of the line dropped.
        let mut bytes = vec![b'z'; 8 * 1024 + 1];
        bytes.extend(b"\nnext\n");
        assert_eq!(logs_of(&bytes), ["z".repeat(8 * 1024), "next".to_owned()]);
    }

    #[test]
    fn a_workers_socket_has_the_buffers_asking_for_4_mib_gets() {
        // 4 MiB each way is asked for; the system may cap it (Linux at net.core.wmem_max and
        // rmem_max), so the core's end is compared with a pair asked for 4 MiB here.
        use std::os::unix::net::UnixStream;
        const ASKED: usize = 4 * 1024 * 1024;
        // Written out, as the log line's limit is: Linux caps both sizes alike, so a smaller
        // request than 4 MiB would read the same below.
        assert_eq!(super::SOCKET_BUFFER, ASKED);
        let dir = scratch("buffers");
        let worker = super::Worker::spawn(&script(&dir, "exec sleep 600")).unwrap();
        let ours = socket2::SockRef::from(&worker.socket);
        let (reference, _other) = UnixStream::pair().unwrap();
        let reference = socket2::SockRef::from(&reference);
        let _ = reference.set_send_buffer_size(ASKED);
        let _ = reference.set_recv_buffer_size(ASKED);
        assert_eq!(
            ours.send_buffer_size().unwrap(),
            reference.send_buffer_size().unwrap()
        );
        assert_eq!(
            ours.recv_buffer_size().unwrap(),
            reference.recv_buffer_size().unwrap()
        );
        // And larger than the platform's default (a pair asked for nothing), so the
        // comparison can fail.
        let (default, _other) = UnixStream::pair().unwrap();
        assert!(
            ours.send_buffer_size().unwrap()
                > socket2::SockRef::from(&default).send_buffer_size().unwrap()
        );
    }

    #[test]
    fn a_render_worker_with_nothing_to_draw_on_is_unavailable_with_its_cause_and_advice() {
        // Refusal::NoAdapter, a fault of the machine, not of the scene and not a bug in
        // Silico: worker_unavailable, its words the cause, its suggestion the refusal's.
        let failure = Failure::new(silico_protocol::Refusal::NoAdapter {
            cause: "no CPU adapter; found [\"Apple M4 Pro\"]".to_owned(),
        });
        let e = failure_error_with_edge_names(WorkerKind::Render, "Render", failure, None);
        assert_eq!(e.code(), ErrorCode::WorkerUnavailable, "{e:?}");
        assert_eq!(
            e.message(),
            "the render worker can't do Render on this machine"
        );
        assert_eq!(
            e.cause(),
            Some(
                "the render worker found nothing it may draw on: no CPU adapter; found [\"Apple M4 Pro\"]"
            )
        );
        assert_eq!(
            e.suggestion(),
            Some("build Silico in its pixi environment, which brings Mesa's lavapipe")
        );
    }
}
