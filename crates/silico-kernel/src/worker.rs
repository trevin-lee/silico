//! The kernel worker (ARCHITECTURE.md, Process model): a process the core starts, which
//! inherits one end of a socket pair as file descriptor [`WORKER_FD`] and serves
//! [`Request`]s on it until the core closes it. Workers keep no state: shapes arrive and
//! leave as B-rep.
//!
//! [`run`] is the whole worker; the production binary passes it [`handle`], and the test
//! worker passes a handler that adds the test operations.

use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use silico_protocol::{
    Blob, Failure, FailureKind, FrameError, FrameReader, Op, Reply, Request, Response, WORKER_FD,
    WORKER_FD_ENV, write_message,
};

use crate::KernelError;

/// The production operations. Test operations get [`FailureKind::UnknownOp`].
pub fn handle(op: Op) -> Result<Reply, Failure> {
    match op {
        Op::MakeBox { dx, dy, dz } => {
            let shape = crate::make_box(dx, dy, dz).map_err(failure)?;
            let brep = crate::to_brep(&shape).map_err(failure)?;
            Ok(Reply::Shape {
                brep: Blob::inline(brep),
            })
        }
        Op::MassProperties { shape } => {
            let shape = crate::from_brep(shape.as_bytes()).map_err(failure)?;
            let props = crate::mass_properties(&shape).map_err(failure)?;
            let center = [props.center.x, props.center.y, props.center.z];
            check_finite(props.volume, center)?;
            Ok(Reply::MassProperties {
                volume: props.volume,
                center,
            })
        }
        Op::Echo { data } => Ok(Reply::Echo { data }),
        Op::DebugSleep { .. }
        | Op::DebugAbort
        | Op::DebugSegfault
        | Op::DebugGarbage
        | Op::DebugEnableFpTraps
        | Op::DebugReplyOutOfTurn { .. } => Err(Failure {
            kind: FailureKind::UnknownOp,
            message: format!(
                "{} is a test operation, not answered by this worker",
                op.name()
            ),
        }),
    }
}

/// A shape OCCT read but can't integrate gives NaN or infinity: never a result.
pub fn check_finite(volume: f64, center: [f64; 3]) -> Result<(), Failure> {
    if volume.is_finite() && center.iter().all(|c| c.is_finite()) {
        return Ok(());
    }
    Err(Failure {
        kind: FailureKind::KernelFailed,
        message: format!("mass properties aren't finite (volume {volume}, center {center:?})"),
    })
}

fn failure(e: KernelError) -> Failure {
    Failure {
        kind: if e.is_invalid_input() {
            FailureKind::InvalidInput
        } else {
            FailureKind::KernelFailed
        },
        message: e.message().to_owned(),
    }
}

/// Fails if any floating-point trap is on. Checked at startup and after every request.
pub fn check_fp_traps() -> Result<(), String> {
    check_fp_trap_mask(crate::fp_trap_mask())
}

/// The decision behind [`check_fp_traps`], on a given trap-enable mask, so it can be
/// tested on every platform (Apple silicon can't switch traps on to test it for real).
pub fn check_fp_trap_mask(mask: u32) -> Result<(), String> {
    match mask {
        0 => Ok(()),
        mask => Err(format!(
            "floating-point traps are on (trap-enable bits {mask:#x})"
        )),
    }
}

/// Serves requests from `socket` until the core closes it. `handle` performs each
/// operation, given the request's id and generation, and may write to the socket directly
/// (the test worker's garbage and out-of-turn operations do). Returns an error if the stream breaks or a request can't be read.
pub fn serve<F>(socket: UnixStream, mut handle: F) -> Result<(), FrameError>
where
    F: FnMut(u64, u64, Op, &mut dyn Write) -> Result<Reply, Failure>,
{
    let mut writer = socket.try_clone().map_err(FrameError::Io)?;
    let mut requests = FrameReader::new(socket);
    while let Some(request) = requests.read_message::<Request>()? {
        let name = request.op.name();
        let span =
            tracing::warn_span!("request", trace = request.trace, id = request.id, op = name);
        let _entered = span.enter();
        let mut result = handle(request.id, request.generation, request.op, &mut writer);
        // Traps switched on during a request would make later results platform-
        // dependent, so the request fails, and the traps are switched back off.
        if let Err(problem) = check_fp_traps() {
            let fixed = crate::disable_fp_traps().is_ok() && check_fp_traps().is_ok();
            tracing::error!("{problem} after {name}; switched off: {fixed}");
            result = Err(Failure {
                kind: FailureKind::Internal,
                message: format!("{problem} after {name}"),
            });
        }
        write_message(
            &mut writer,
            &Response {
                id: request.id,
                generation: request.generation,
                result,
            },
        )?;
    }
    Ok(())
}

/// The socket the core passed this process, by the descriptor named in [`WORKER_FD_ENV`].
fn inherited_socket() -> Result<UnixStream, String> {
    let value = std::env::var(WORKER_FD_ENV)
        .map_err(|_| format!("{WORKER_FD_ENV} isn't set: workers are started by silico"))?;
    let fd: i32 = value
        .parse()
        .map_err(|_| format!("{WORKER_FD_ENV}={value:?} isn't a file descriptor"))?;
    if fd != WORKER_FD {
        return Err(format!("{WORKER_FD_ENV}={fd}, expected {WORKER_FD}"));
    }
    // SAFETY: the core passes this descriptor to the worker for the worker's exclusive
    // use (WORKER_FD_ENV says so), and nothing else in this process opens or closes it.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    match file.metadata() {
        Ok(meta) if meta.file_type().is_socket() => Ok(UnixStream::from(OwnedFd::from(file))),
        Ok(_) => Err(format!("descriptor {fd} isn't a socket")),
        Err(e) => {
            // Not an open descriptor: don't let the drop close a number this process
            // doesn't own.
            std::mem::forget(file);
            Err(format!("descriptor {fd} isn't usable: {e}"))
        }
    }
}

/// The worker process: logging to stderr as JSON lines (filtered by `SILICO_LOG`), which
/// the core re-emits at each line's own level with the request's trace; floating-point
/// traps off (or refuse to run); then serve the inherited socket.
pub fn run<F>(handle: F) -> ExitCode
where
    F: FnMut(u64, u64, Op, &mut dyn Write) -> Result<Reply, Failure>,
{
    let filter = tracing_subscriber::EnvFilter::try_from_env("SILICO_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();

    if let Err(e) = crate::disable_fp_traps() {
        tracing::error!("couldn't switch floating-point traps off: {e}");
        return ExitCode::from(3);
    }
    if let Err(problem) = check_fp_traps() {
        tracing::error!("{problem}; refusing to run");
        return ExitCode::from(3);
    }
    let socket = match inherited_socket() {
        Ok(socket) => socket,
        Err(problem) => {
            tracing::error!("{problem}");
            return ExitCode::from(2);
        }
    };
    tracing::debug!("worker ready");
    match serve(socket, handle) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("stopping: {e}");
            ExitCode::FAILURE
        }
    }
}
