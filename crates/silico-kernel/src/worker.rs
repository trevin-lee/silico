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
    Blob, BuiltData, Failure, FailureKind, FrameError, FrameReader, Generation, Op, Reply, Request,
    RequestId, Response, ShapeData, ShapeSummary, WORKER_FD, WORKER_FD_ENV, write_message,
};

use crate::{KernelError, KernelErrorKind};

/// The production operations. Test operations get [`FailureKind::UnknownOp`].
pub fn handle(op: Op) -> Result<Reply, Failure> {
    match op {
        Op::MakePrimitive(primitive) => {
            let shape = crate::make(&primitive).map_err(failure)?;
            shape_reply(&shape)
        }
        Op::Summarize { shape } => {
            let shape = shape_of(&shape)?;
            let summary = crate::summarize(&shape).map_err(failure)?;
            check_finite(&summary)?;
            Ok(Reply::Summary(summary))
        }
        Op::Fillet {
            shape,
            edges,
            radius,
        } => {
            let shape = shape_of(&shape)?;
            let rounded = crate::fillet(&shape, &edges, radius).map_err(failure)?;
            built_reply(rounded)
        }
        Op::Translate { shape, offset } => {
            let shape = shape_of(&shape)?;
            built_reply(crate::translate(&shape, offset).map_err(failure)?)
        }
        Op::Extrude {
            profile,
            length,
            direction,
        } => built_reply(crate::extrude(&profile, length, direction).map_err(failure)?),
        Op::Revolve {
            profile,
            axis,
            angle,
        } => built_reply(crate::revolve(&profile, &axis, angle).map_err(failure)?),
        Op::Boolean { op, base, tool } => {
            let base = shape_of(&base)?;
            let tool = shape_of(&tool)?;
            built_reply(crate::boolean(op, &base, &tool).map_err(failure)?)
        }
        Op::Tessellate { shape, deflection } => {
            let shape = shape_of(&shape)?;
            let mesh = crate::tessellate(&shape, deflection).map_err(failure)?;
            Ok(Reply::Mesh(mesh))
        }
        Op::Echo { data } => Ok(Reply::Echo { data }),
        Op::Debug(_) => Err(Failure::new(
            FailureKind::UnknownOp,
            format!(
                "{} is a test operation, not answered by this worker",
                op.name()
            ),
        )),
    }
}

/// A shape from a request, with the validity it came with, if a worker checked it (if
/// not, it's checked when it's first needed).
fn shape_of(data: &ShapeData) -> Result<crate::Shape, Failure> {
    let shape = crate::from_brep(data.brep().as_bytes()).map_err(failure)?;
    Ok(match data.valid() {
        Some(valid) => shape.with_validity(valid),
        None => shape,
    })
}

/// A shape to send back, with its validity (checked now if it wasn't already).
fn shape_data(shape: &crate::Shape) -> Result<ShapeData, Failure> {
    Ok(ShapeData::checked(
        Blob::inline(crate::to_brep(shape).map_err(failure)?),
        shape.is_valid().map_err(failure)?,
    ))
}

fn shape_reply(shape: &crate::Shape) -> Result<Reply, Failure> {
    Ok(Reply::Shape(shape_data(shape)?))
}

/// An operation's result and its history.
fn built_reply(built: crate::Built) -> Result<Reply, Failure> {
    Ok(Reply::Built(BuiltData {
        shape: shape_data(&built.shape)?,
        history: built.history,
    }))
}

/// A shape OCCT read but can't integrate gives NaN or infinity: never a result.
fn check_finite(summary: &ShapeSummary) -> Result<(), Failure> {
    let [x, y, z] = summary.center;
    let values = [
        ("volume", summary.volume),
        ("area", summary.area),
        ("center x", x),
        ("center y", y),
        ("center z", z),
    ];
    match values.into_iter().find(|(_, value)| !value.is_finite()) {
        None => Ok(()),
        Some((name, value)) => Err(Failure::new(
            FailureKind::KernelFailed,
            format!("the shape's {name} isn't finite ({value})"),
        )),
    }
}

fn failure(e: KernelError) -> Failure {
    Failure {
        kind: match e.kind() {
            KernelErrorKind::InvalidInput => FailureKind::InvalidInput,
            KernelErrorKind::Failed => FailureKind::KernelFailed,
            KernelErrorKind::InvalidResult => FailureKind::InvalidResult,
            KernelErrorKind::Internal => FailureKind::Internal,
        },
        message: e.message().to_owned(),
        element: e.element().map(str::to_owned),
        suggestion: e.suggestion().map(str::to_owned),
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
/// (the test worker's garbage and out-of-turn operations do). Returns an error if the
/// stream breaks or a request can't be read.
pub fn serve<F>(socket: UnixStream, mut handle: F) -> Result<(), FrameError>
where
    F: FnMut(RequestId, Generation, Op, &mut dyn Write) -> Result<Reply, Failure>,
{
    let mut writer = socket.try_clone().map_err(FrameError::Io)?;
    let mut requests = FrameReader::new(socket);
    while let Some(request) = requests.read_message::<Request>()? {
        let name = request.op.name();
        let span = tracing::warn_span!(
            "request",
            trace = request.trace.0,
            id = request.id.0,
            op = name
        );
        let _entered = span.enter();
        let mut result = handle(request.id, request.generation, request.op, &mut writer);
        // Traps switched on during a request would make later results platform-
        // dependent, so the request fails, and the traps are switched back off.
        if let Err(problem) = check_fp_traps() {
            let fixed = crate::disable_fp_traps().is_ok() && check_fp_traps().is_ok();
            tracing::error!("{problem} after {name}; switched off: {fixed}");
            result = Err(Failure::new(
                FailureKind::Internal,
                format!("{problem} after {name}"),
            ));
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
    F: FnMut(RequestId, Generation, Op, &mut dyn Write) -> Result<Reply, Failure>,
{
    // The app reports a SILICO_LOG it can't use (silico_core::logging); a worker, which
    // inherits it, quietly uses the same default.
    let filter = std::env::var("SILICO_LOG")
        .ok()
        .filter(|value| !value.is_empty())
        .and_then(|value| tracing_subscriber::EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new("warn"));
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

#[cfg(test)]
mod tests {
    #[test]
    fn each_kind_of_kernel_error_reaches_the_app_as_its_own_failure() {
        use silico_protocol::FailureKind;

        use crate::{KernelError, KernelErrorKind};
        for (kind, want) in [
            (KernelErrorKind::InvalidInput, FailureKind::InvalidInput),
            (KernelErrorKind::Failed, FailureKind::KernelFailed),
            (KernelErrorKind::InvalidResult, FailureKind::InvalidResult),
            (KernelErrorKind::Internal, FailureKind::Internal),
        ] {
            let e = KernelError::of_kind(kind, "words").with_suggestion("try this");
            let f = super::failure(e);
            assert_eq!(f.kind, want, "{kind:?}");
            assert_eq!(f.message, "words");
            assert_eq!(f.suggestion.as_deref(), Some("try this"));
        }
    }

    use silico_protocol::{FailureKind, ShapeSummary};

    use super::check_finite;

    #[test]
    fn a_summary_that_isnt_finite_is_refused_naming_the_value() {
        let good = ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            valid: true,
        };
        assert!(check_finite(&good).is_ok());
        type Spoil = fn(&mut ShapeSummary);
        let cases: [(&str, Spoil); 6] = [
            ("volume isn't finite (NaN)", |s| s.volume = f64::NAN),
            ("volume isn't finite (inf)", |s| s.volume = f64::INFINITY),
            ("area isn't finite (NaN)", |s| s.area = f64::NAN),
            ("center x isn't finite (NaN)", |s| s.center[0] = f64::NAN),
            ("center y isn't finite (-inf)", |s| {
                s.center[1] = f64::NEG_INFINITY
            }),
            ("center z isn't finite (inf)", |s| {
                s.center[2] = f64::INFINITY
            }),
        ];
        for (want, spoil) in cases {
            let mut bad = good.clone();
            spoil(&mut bad);
            let failure = check_finite(&bad).unwrap_err();
            assert_eq!(failure.kind, FailureKind::KernelFailed);
            assert_eq!(failure.message, format!("the shape's {want}"));
        }
    }
}
