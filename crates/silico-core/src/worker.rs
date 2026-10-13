//! The core's side of a kernel worker (ARCHITECTURE.md, Process model): it starts the
//! worker with one end of a socket pair, sends requests, and waits for replies with a
//! timeout covering both sending and waiting. A worker that crashes, hangs, or replies
//! with something unreadable is killed and reaped, and the call returns a structured
//! error; the app keeps running.
//!
//! Plain threads, no async runtime: one thread reads the worker's replies, and one
//! re-emits its log lines (JSON, one per line) into `tracing` at their own level, keeping
//! the last lines to attach to a crash report. Each call has a trace ID, which the worker
//! records on its logs for the request, so they line up with the core's span for the call.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use command_fds::{CommandFdExt, FdMapping};
use silico_protocol::{
    ElementRef, Failure, FailureKind, FrameError, FrameReader, Generation, Op, Refusal, Reply,
    Request, RequestId, Response, TraceId, WORKER_FD, WORKER_FD_ENV, decode, write_message,
};

use crate::{Error, ErrorCode};

/// How many of a worker's last log lines a crash report carries.
const STDERR_TAIL_LINES: usize = 20;

/// The longest log line kept; the rest of a longer line is dropped, so a worker can't make
/// the core buffer without limit.
const MAX_LOG_LINE: usize = 8 * 1024;

/// How long a worker whose socket closed gets to exit on its own before it's killed, and
/// how long its log thread then gets to finish.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// The least a crashed call waits for its worker to exit, past the call's deadline.
const MIN_EXIT_GRACE: Duration = Duration::from_millis(100);

/// The socket buffer size asked for on each end.
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
/// Workers close what they inherit when they start running (`silico_kernel::worker`), so
/// such a descriptor would be held only until then; but until then is as long as the
/// system takes to launch the worker (macOS has held a newly copied binary for 17 s and
/// more), and that spawn would wait as long. Starting workers one at a time closes the gap;
/// it costs a pool of fourteen a few milliseconds at start.
static SPAWNING: Mutex<()> = Mutex::new(());

/// A running kernel worker. Dropping it kills and reaps the process.
pub struct Worker {
    /// Shared with [`WorkerKiller`]s, behind a lock taken for every kill and every reap, so a
    /// killer never signals a process that's been reaped (whose ID may be someone else's).
    child: Arc<Mutex<Child>>,
    socket: UnixStream,
    replies: Receiver<Result<Response, FrameError>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Closes when the log thread has read everything the worker wrote.
    stderr_done: Receiver<()>,
    /// Set when the worker says it has started (see [`silico_protocol::write_started`]), or
    /// replies.
    started: Arc<AtomicBool>,
    /// How many requests the worker has answered (with a result or a refusal).
    answers: u64,
    next_id: u64,
    /// Set once the worker has crashed, hung, or misbehaved and been stopped.
    stopped: bool,
}

impl Worker {
    /// Starts the worker executable at `exe`.
    pub fn spawn(exe: &Path) -> Result<Self, Error> {
        let spawning = lock(&SPAWNING);
        let (ours, theirs) = UnixStream::pair().map_err(|e| start_failed("socket pair", &e))?;
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
            .map_err(|e| internal("passing the socket", e))?;
        let child = command.spawn().map_err(|e| {
            let message = format!("couldn't start the kernel worker {}", exe.display());
            // Short of something, the worker never ran; otherwise it can't be run.
            shortage(&e, &message).unwrap_or_else(|| {
                Error::new(ErrorCode::WorkerCrashed, message)
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
        let (done_tx, stderr_done) = mpsc::channel::<()>();
        if let Some(stderr) = guard.take_stderr() {
            let tail = Arc::clone(&stderr_tail);
            std::thread::Builder::new()
                .name(format!("worker-{pid}-logs"))
                .spawn(move || {
                    forward_logs(stderr, pid, &tail);
                    drop(done_tx);
                })
                .map_err(|e| start_failed("starting the log thread", &e))?;
        }

        // Non-blocking, so a send can give up at its deadline (see `DeadlineWriter`).
        ours.set_nonblocking(true)
            .map_err(|e| start_failed("making the socket non-blocking", &e))?;
        let reader = ours
            .try_clone()
            .map_err(|e| start_failed("socket clone", &e))?;
        // One reply in flight: a worker sending unrequested replies can't make the core
        // buffer them without limit.
        let (tx, replies) = mpsc::sync_channel(1);
        let started = Arc::new(AtomicBool::new(false));
        let has_started = Arc::clone(&started);
        std::thread::Builder::new()
            .name(format!("worker-{pid}-replies"))
            .spawn(move || {
                let mut frames = FrameReader::new(PatientReader(reader));
                loop {
                    let reply = match frames.read_frame() {
                        // The worker has started (any frame says so); an empty one says only that.
                        Ok(Some(frame)) => {
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
            .map_err(|e| start_failed("starting the reply thread", &e))?;

        let child = guard
            .release()
            .ok_or_else(|| internal("keeping the process", "it was lost"))?;
        tracing::debug!(pid, exe = %exe.display(), "started kernel worker");
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            socket: ours,
            replies,
            stderr_tail,
            stderr_done,
            started,
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
                    "can't send {name}: this kernel worker was stopped after an earlier failure"
                ),
            )
            .with_suggestion("start a new kernel worker"));
        }
        // A timeout too long to add to the clock has no deadline.
        let deadline = Instant::now().checked_add(timeout);
        let id = RequestId(self.next_id);
        self.next_id += 1;
        let trace = TraceId(NEXT_TRACE.fetch_add(1, Ordering::Relaxed));
        let span = tracing::debug_span!(
            "worker_call",
            trace = trace.0,
            pid = self.pid(),
            id = id.0,
            generation = generation.0,
            op = name
        );
        let _entered = span.enter();

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
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!("{name} couldn't be sent to the kernel worker"),
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
        match reply {
            Ok(Ok(response)) if response.id == id && response.generation == generation => {
                self.answers += 1;
                match response.result {
                    Ok(reply) => Ok(reply),
                    // Checked before it's described, so a refusal the request can't explain
                    // (a broken worker's) costs no more than the request does.
                    Err(failure) => match refusal_fits(&failure.refusal, listed.as_deref()) {
                        Ok(()) => Err(failure_error(name, failure)),
                        Err(misfit) => Err(self.stop(
                            Error::new(
                                ErrorCode::ProtocolError,
                                format!(
                                    "the kernel worker's refusal of {name} doesn't fit the request"
                                ),
                            )
                            .with_cause(misfit)
                            .with_suggestion(BROKEN_WORKER),
                        )),
                    },
                }
            }
            Ok(Ok(response)) => Err(self.stop(
                Error::new(
                    ErrorCode::ProtocolError,
                    format!("the kernel worker answered {name} out of turn"),
                )
                .with_cause(format!(
                    "reply for request {} (generation {}), expected {} (generation {})",
                    response.id.0, response.generation.0, id.0, generation.0
                ))
                .with_suggestion(BROKEN_WORKER),
            )),
            Ok(Err(FrameError::Io(_) | FrameError::Truncated { .. }))
            | Err(RecvTimeoutError::Disconnected) => Err(self.crashed(name, deadline, true, None)),
            Ok(Err(e)) => Err(self.stop(
                Error::new(
                    ErrorCode::ProtocolError,
                    format!("the kernel worker's reply to {name} couldn't be read"),
                )
                .with_cause(e.to_string())
                .with_suggestion(BROKEN_WORKER),
            )),
            Err(RecvTimeoutError::Timeout) => Err(self.timed_out(name, timeout)),
        }
    }

    fn timed_out(&mut self, name: &str, timeout: Duration) -> Error {
        let error = Error::new(
            ErrorCode::WorkerTimeout,
            format!("the kernel worker didn't finish {name} within {timeout:?}"),
        );
        // A worker that hadn't started never had the request to work on.
        let error = if self.has_started() {
            error.with_suggestion("simplify the operation, or allow it more time")
        } else {
            error
                .with_cause("the kernel worker hadn't started")
                .with_suggestion(
                    "try the request again: the kernel worker hadn't started in the time \
                     allowed (a new worker binary's first launch can be slow while the system \
                     checks it); if it never starts, check Silico's install",
                )
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
        // lines before the report is built.
        let _ = self
            .stderr_done
            .recv_timeout(until.saturating_duration_since(Instant::now()));
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
        Error::new(
            ErrorCode::WorkerCrashed,
            format!("the kernel worker crashed during {name}"),
        )
        .with_cause(cause)
        .with_suggestion(crash_advice(status, reached))
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

/// Re-emits each of the worker's log lines into `tracing` at its own level, and keeps the
/// last [`STDERR_TAIL_LINES`] for crash reports. Lines are read as bytes, so text that
/// isn't UTF-8 can't stop the reading, and cut at [`MAX_LOG_LINE`].
fn forward_logs(stderr: impl Read, pid: u32, tail: &Mutex<VecDeque<String>>) {
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
        let summary = emit_worker_log(text, pid);
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(summary);
    }
}

/// Emits one worker log line (JSON, from `tracing_subscriber`'s json format) at its own
/// level, and returns it as `LEVEL message` for crash reports. Anything that isn't such a
/// line (a crash message from the C runtime, say) is kept as is, at warn.
fn emit_worker_log(line: &str, pid: u32) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(line).ok();
    let Some(event) = parsed.filter(serde_json::Value::is_object) else {
        tracing::warn!(target: "silico::worker", pid, "{line}");
        return line.to_owned();
    };
    let level = event["level"].as_str().unwrap_or("INFO");
    let message = event["fields"]["message"].as_str().unwrap_or(line);
    let trace = event["span"]["trace"].as_u64();
    match level {
        "ERROR" => tracing::error!(target: "silico::worker", pid, trace, "{message}"),
        "WARN" => tracing::warn!(target: "silico::worker", pid, trace, "{message}"),
        "INFO" => tracing::info!(target: "silico::worker", pid, trace, "{message}"),
        "DEBUG" => tracing::debug!(target: "silico::worker", pid, trace, "{message}"),
        _ => tracing::trace!(target: "silico::worker", pid, trace, "{message}"),
    }
    format!("{level} {message}")
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
fn crash_advice(status: Option<ExitStatus>, reached: Reached) -> &'static str {
    let request = matches!(reached, Reached::Request);
    match status.map(|s| (s.code(), s.signal())) {
        Some((Some(_), _)) => {
            "check Silico's install: the kernel worker exited on its own (see its last output), \
             which no change to the request will help; report it if it persists"
        }
        Some((None, Some(signal)))
            if matches!(reached, Reached::NotStarted) && FAULTS.contains(&signal) =>
        {
            "check Silico's install: the kernel worker crashed as it started, before it could \
             read the request (see its last output); report it if it persists"
        }
        Some((None, Some(signal))) if request && FAULTS.contains(&signal) => {
            "change the operation's values: these crashed the kernel, and the same request \
             would crash it again"
        }
        Some((None, Some(libc::SIGKILL))) if matches!(reached, Reached::NotStarted) => {
            "try the request again: the kernel worker was killed before it started; if it's \
             killed before starting again, check Silico's install (macOS kills a worker binary \
             whose code signature is broken)"
        }
        Some((None, Some(libc::SIGKILL))) if request => {
            "try the request once more: the kernel worker was killed, most likely by the \
             system for running out of memory; if it's killed again, the request needs too \
             much memory: simplify it"
        }
        _ => {
            "try the request again: the kernel worker was stopped from outside, stopped \
             responding, or was gone before the request reached it, not by these values"
        }
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
const BROKEN_WORKER: &str = "this is a bug in Silico's kernel worker, not in the request: report \
     it (the worker was stopped, and the next request starts another)";

/// What to try after an internal error: a bug, not the request's values.
pub(crate) const SILICO_BUG: &str = "this is a bug in Silico, not in the request: report it";

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
        Refusal::NoEdges { .. } => {
            return match listed {
                Some([]) => Ok(()),
                _ => Err("it says no edges were listed, but some were".to_owned()),
            };
        }
        Refusal::FaceUnmeshed { face: 0 } => return Err("it names face 0".to_owned()),
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

/// A worker's refusal or failure, as a structured error. The core names the elements at
/// fault (as FreeCAD does, `Edge3`, until a document's names arrive) and has the refusal
/// described with those names (`Refusal::describe`). A kernel failure always suggests
/// something.
fn failure_error(op: &str, failure: Failure) -> Error {
    let name = |element: ElementRef| element.to_string();
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
        FailureKind::UnknownOp | FailureKind::Internal => (
            Error::new(
                ErrorCode::Internal,
                format!("the kernel worker couldn't do {op}"),
            )
            .with_cause(why),
            SILICO_BUG,
        ),
    };
    let error = error.with_elements(failure.refusal.elements().into_iter().map(name));
    let error = match failure.refusal.parameter() {
        Some(parameter) => error.with_parameter(parameter),
        None => error,
    };
    error.with_suggestion(described.suggestion.unwrap_or_else(|| fallback.to_owned()))
}

fn internal(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("couldn't start the kernel worker: {what} failed"),
    )
    .with_cause(e.to_string())
    .with_suggestion(SILICO_BUG)
}

/// A step of starting a worker failed in this process: short of descriptors, threads, or
/// memory, most likely.
fn start_failed(what: &str, e: &io::Error) -> Error {
    shortage(
        e,
        &format!("couldn't start the kernel worker: {what} failed"),
    )
    .unwrap_or_else(|| internal(what, e))
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use silico_protocol::Failure;

    use super::{
        FAULTS, KillOnDrop, MAX_LOG_LINE, STDERR_TAIL_LINES, crash_advice, describe_exit,
        failure_error, forward_logs, signal_name,
    };
    use crate::ErrorCode;

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
                let advice = crash_advice(signalled(signal), reached);
                assert!(advice.starts_with(says), "{signal} {reached:?}: {advice}");
            }
        }
        for (reached, says) in [
            (Request, "try the request once more"),
            (Started, again),
            (NotStarted, again),
        ] {
            let advice = crash_advice(signalled(libc::SIGKILL), reached);
            assert!(advice.starts_with(says), "{reached:?}: {advice}");
        }
        // Killed before it started: from outside, or, again and again, the system refusing
        // the binary.
        let unstarted = crash_advice(signalled(libc::SIGKILL), NotStarted);
        assert!(unstarted.contains(install), "{unstarted}");
        assert!(!crash_advice(signalled(libc::SIGKILL), Started).contains(install));
        for reached in [Request, Started, NotStarted] {
            for signal in [
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGHUP,
                libc::SIGUSR1,
                libc::SIGPIPE,
            ] {
                let advice = crash_advice(signalled(signal), reached);
                assert!(advice.starts_with(again), "{signal} {reached:?}: {advice}");
            }
            // Killed after it stopped responding: no status.
            assert!(
                crash_advice(None, reached).starts_with(again),
                "{reached:?}"
            );
            // An exit code (3, in the status's high byte) is the install's, either way.
            let exited = crash_advice(Some(ExitStatus::from_raw(3 << 8)), reached);
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
        assert_eq!(fits(&Refusal::FaceUnmeshed { face: 1 }, None), Ok(()));
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
        assert!(fits(&Refusal::FaceUnmeshed { face: 0 }, None).is_err());
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
        forward_logs(bytes, 1, &tail);
        tail.into_inner().unwrap().into_iter().collect()
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
            tracing::subscriber::with_default(subscriber, || super::emit_worker_log(&line, 1));
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
}
