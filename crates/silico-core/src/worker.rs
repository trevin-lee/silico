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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use command_fds::{CommandFdExt, FdMapping};
use silico_protocol::{
    Failure, FailureKind, FrameError, FrameReader, Generation, Op, Reply, Request, RequestId,
    Response, TraceId, WORKER_FD, WORKER_FD_ENV, write_message,
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
    next_id: u64,
    /// Set once the worker has crashed, hung, or misbehaved and been stopped.
    stopped: bool,
}

impl Worker {
    /// Starts the worker executable at `exe`.
    pub fn spawn(exe: &Path) -> Result<Self, Error> {
        let spawning = lock(&SPAWNING);
        let (ours, theirs) = UnixStream::pair().map_err(|e| internal("socket pair", e))?;
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
            Error::new(
                ErrorCode::WorkerCrashed,
                format!("couldn't start the kernel worker {}", exe.display()),
            )
            .with_cause(e.to_string())
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
                .map_err(|e| internal("starting the log thread", e))?;
        }

        // Non-blocking, so a send can give up at its deadline (see `DeadlineWriter`).
        ours.set_nonblocking(true)
            .map_err(|e| internal("making the socket non-blocking", e))?;
        let reader = ours.try_clone().map_err(|e| internal("socket clone", e))?;
        // One reply in flight: a worker sending unrequested replies can't make the core
        // buffer them without limit.
        let (tx, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("worker-{pid}-replies"))
            .spawn(move || {
                let mut frames = FrameReader::new(PatientReader(reader));
                loop {
                    match frames.read_message::<Response>() {
                        Ok(Some(response)) => {
                            if tx.send(Ok(response)).is_err() {
                                break;
                            }
                        }
                        // The socket closed: dropping the sender tells the caller.
                        Ok(None) => break,
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                }
            })
            .map_err(|e| internal("starting the reply thread", e))?;

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

    /// Whether the worker has sent something no call asked for (a second reply to an
    /// earlier request, say), which is read and dropped. Such a worker can't be trusted:
    /// its next call would fail on that reply, out of turn.
    pub fn has_stray_reply(&mut self) -> bool {
        self.replies.try_recv().is_ok()
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
        };
        match write_message(&mut writer, &request) {
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
            Err(e) => return Err(self.crashed(name, Some(e.to_string()))),
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
            Ok(Ok(response)) if response.id == id && response.generation == generation => response
                .result
                .map_err(|failure| failure_error(name, failure)),
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
            | Err(RecvTimeoutError::Disconnected) => Err(self.crashed(name, None)),
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
        self.stop(
            Error::new(
                ErrorCode::WorkerTimeout,
                format!("the kernel worker didn't finish {name} within {timeout:?}"),
            )
            .with_suggestion("simplify the operation, or allow it more time"),
        )
    }

    /// The worker's socket closed: report how it ended, with its last log lines.
    fn crashed(&mut self, name: &str, write_error: Option<String>) -> Error {
        let status = self.wait_for_exit();
        let delivered = write_error.is_none();
        // The worker has exited, so its log pipe closes; let the log thread read the last
        // lines before the report is built.
        let _ = self.stderr_done.recv_timeout(EXIT_GRACE);
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
        .with_suggestion(crash_advice(status, delivered))
    }

    /// Waits briefly for a worker whose socket closed to exit, then kills it if needed.
    fn wait_for_exit(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + EXIT_GRACE;
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
}

impl io::Write for DeadlineWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            match (&*self.socket).write(bytes) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
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

/// How the worker process ended, naming common signals by this platform's numbering.
/// What to try after a worker crashed, by how it ended: a crash of its own (a signal such
/// as SIGSEGV or SIGABRT) while it had the request is the request's values, which would
/// crash it again; a kill from outside (SIGKILL: out of memory, say), a worker that stopped
/// responding, or one that was gone before the request reached it is worth trying again
/// (the next request starts another); a worker that exited with a code ended on its own,
/// which is its install, not the request.
fn crash_advice(status: Option<ExitStatus>, delivered: bool) -> &'static str {
    match status.map(|s| (s.code(), s.signal())) {
        Some((Some(_), _)) => {
            "check Silico's install: the kernel worker exited on its own (see its last output), \
             which no change to the request will help; report it if it persists"
        }
        Some((None, Some(signal))) if delivered && signal != libc::SIGKILL => {
            "change the operation's values: these crashed the kernel, and the same request \
             would crash it again"
        }
        _ => {
            "try the request again: the kernel worker was stopped from outside (out of memory, \
             say), stopped responding, or was gone before the request reached it, not by these \
             values"
        }
    }
}

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
    ]
    .into_iter()
    .find(|(number, _)| *number == signal)
    .map(|(_, name)| name)
}

/// What to try when the worker itself misbehaved: nothing about the request will help.
const BROKEN_WORKER: &str = "this is a bug in Silico's kernel worker, not in the request: report \
     it (the worker was stopped, and the next request starts another)";

/// What to try after an internal error: a bug, not the request's values.
const SILICO_BUG: &str = "this is a bug in Silico, not in the request: report it";

/// A worker's refusal or failure, as a structured error, naming the element at fault and
/// what to try instead when the worker knew them. A kernel failure always suggests
/// something.
fn failure_error(op: &str, failure: Failure) -> Error {
    let (error, fallback) = match failure.kind {
        FailureKind::InvalidInput => (
            Error::new(ErrorCode::InvalidInput, failure.message),
            Some("change the value the message names"),
        ),
        FailureKind::KernelFailed => (
            Error::new(ErrorCode::KernelFailed, format!("{op} failed")).with_cause(failure.message),
            Some("change the operation's values: the kernel can't build it from these"),
        ),
        FailureKind::InvalidResult => (
            Error::new(
                ErrorCode::InvalidResult,
                format!("{op} gave an invalid result, which was refused"),
            )
            .with_cause(failure.message),
            Some("change the operation's values: the kernel can't build a valid result from these"),
        ),
        FailureKind::UnknownOp | FailureKind::Internal => (
            Error::new(
                ErrorCode::Internal,
                format!("the kernel worker couldn't do {op}"),
            )
            .with_cause(failure.message),
            Some(SILICO_BUG),
        ),
    };
    let error = match failure.element {
        Some(element) => error.with_element(element),
        None => error,
    };
    match failure.suggestion.as_deref().or(fallback) {
        Some(suggestion) => error.with_suggestion(suggestion),
        None => error,
    }
}

fn internal(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("couldn't start the kernel worker: {what} failed"),
    )
    .with_cause(e.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use silico_protocol::{Failure, FailureKind};

    use super::{
        KillOnDrop, MAX_LOG_LINE, STDERR_TAIL_LINES, describe_exit, failure_error, forward_logs,
    };
    use crate::ErrorCode;

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
    fn kernel_failures_get_their_own_code_and_always_a_suggestion() {
        let failure = |kind, suggestion: Option<&str>| Failure {
            kind,
            message: "the kernel's words".to_owned(),
            element: Some("Edge1".to_owned()),
            suggestion: suggestion.map(str::to_owned),
        };
        let cases = [
            (
                FailureKind::KernelFailed,
                ErrorCode::KernelFailed,
                "Fillet failed",
                "change the operation's values: the kernel can't build it from these",
            ),
            (
                FailureKind::InvalidResult,
                ErrorCode::InvalidResult,
                "Fillet gave an invalid result, which was refused",
                "change the operation's values: the kernel can't build a valid result from these",
            ),
        ];
        for (kind, code, message, fallback) in cases {
            let error = failure_error("Fillet", failure(kind, None));
            assert_eq!(
                (
                    error.code(),
                    error.message(),
                    error.cause(),
                    error.element()
                ),
                (code, message, Some("the kernel's words"), Some("Edge1"))
            );
            assert_eq!(error.suggestion(), Some(fallback));
            // The worker's own suggestion, when it has one, comes first.
            let error = failure_error("Fillet", failure(kind, Some("try a smaller radius")));
            assert_eq!(error.suggestion(), Some("try a smaller radius"));
        }
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
