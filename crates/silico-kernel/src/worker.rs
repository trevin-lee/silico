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
    Blob, BuiltData, Failure, FrameError, FrameReader, FreecadMeasures, Generation, Op,
    REQUEST_DONE, Refusal, Reply, Request, RequestId, Response, ShapeData, ShapeSummary, WORKER_FD,
    WORKER_FD_ENV, write_message, write_started,
};

use crate::KernelError;

/// The production operations. Test operations get [`Refusal::TestOperation`].
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
        Op::MeasureAsFreecad { shape } => {
            let shape = shape_of(&shape)?;
            let measures = crate::measure_as_freecad(&shape).map_err(failure)?;
            check_freecad_finite(&measures)?;
            Ok(Reply::FreecadMeasures(measures))
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
        Op::Chamfer {
            shape,
            edges,
            size,
            flip,
        } => {
            let shape = shape_of(&shape)?;
            built_reply(crate::chamfer(&shape, &edges, size, flip).map_err(failure)?)
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
        Op::Debug(_) => Err(Failure::new(Refusal::TestOperation {
            op: op.name().to_owned(),
        })),
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
    all_finite([
        ("volume", summary.volume),
        ("area", summary.area),
        ("center x", x),
        ("center y", y),
        ("center z", z),
    ])
}

/// As [`check_finite`], for a shape measured as FreeCAD measures it, with its box.
fn check_freecad_finite(m: &FreecadMeasures) -> Result<(), Failure> {
    let [x, y, z] = m.center;
    let [low, high] = m.bounds.unwrap_or_default();
    all_finite([
        ("volume", m.volume),
        ("area", m.area),
        ("center x", x),
        ("center y", y),
        ("center z", z),
        ("bounding box's lowest x", low[0]),
        ("bounding box's lowest y", low[1]),
        ("bounding box's lowest z", low[2]),
        ("bounding box's highest x", high[0]),
        ("bounding box's highest y", high[1]),
        ("bounding box's highest z", high[2]),
    ])
}

fn all_finite<const N: usize>(values: [(&str, f64); N]) -> Result<(), Failure> {
    match values.into_iter().find(|(_, value)| !value.is_finite()) {
        None => Ok(()),
        Some((quantity, value)) => Err(Failure::new(Refusal::NotFinite {
            quantity: quantity.to_owned(),
            value,
        })),
    }
}

/// What the worker sends for `e`: why, as data, and OCCT's words.
fn failure(e: KernelError) -> Failure {
    Failure {
        refusal: e.refusal().clone(),
        cause: e.cause().map(str::to_owned),
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
    write_started(&mut writer)?;
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
        let mask = crate::fp_trap_mask();
        if let Err(problem) = check_fp_traps() {
            let fixed = crate::disable_fp_traps().is_ok() && check_fp_traps().is_ok();
            tracing::error!("{problem} after {name}; switched off: {fixed}");
            result = Err(Failure::new(Refusal::FpTrapsOn {
                mask,
                op: name.to_owned(),
            }));
        }
        // The request's last log line, after any it logged and before its reply: the core
        // keeps the span the request was asked in until it has read this, so every line of
        // the request goes under it, and then lets it go.
        {
            let mut log = std::io::stderr().lock();
            let _ = writeln!(log, "{{\"{REQUEST_DONE}\":{}}}", request.trace.0);
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

/// Closes every descriptor the worker inherited above its socket ([`WORKER_FD`]). The app
/// may hold descriptors not marked close-on-exec (a library's, or a test runner's jobserver
/// pipe), and a worker would keep them for its life, holding open pipes others wait on.
/// Runs first, before anything in this process has opened a descriptor of its own.
fn close_inherited_descriptors() {
    let listing = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    let Ok(entries) = std::fs::read_dir(listing) else {
        return;
    };
    // Read whole first: the listing's own descriptor is among these, and is closed (with the
    // listing) before the loop, so closing its number again fails harmlessly.
    let fds: Vec<i32> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    for fd in fds.into_iter().filter(|&fd| fd > WORKER_FD) {
        // SAFETY: nothing in this process owns a descriptor above WORKER_FD yet, so each is
        // one it inherited; close only ends this process's use of it.
        unsafe {
            libc::close(fd);
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
    close_inherited_descriptors();
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
    fn a_kernel_error_reaches_the_app_as_its_refusal_and_occts_words_only() {
        use silico_protocol::{DressingKind, EdgeTrouble, Operation, Refusal, Trouble};

        use crate::KernelError;
        let refusals = [
            Refusal::EdgeMissing {
                edge: 13,
                count: 12,
            },
            Refusal::EdgesRefused {
                dressing: DressingKind::Fillet,
                troubles: vec![EdgeTrouble {
                    edge: 3,
                    trouble: Trouble::Smooth,
                }],
            },
            Refusal::ResultInvalid {
                operation: Operation::Extrusion,
                edges: Vec::new(),
            },
        ];
        for refusal in refusals {
            let f = super::failure(KernelError::new(refusal.clone()));
            assert_eq!((f.refusal, f.cause), (refusal, None));
        }
        let occt = Refusal::Occt {
            operation: Operation::Other,
            edges: Vec::new(),
        };
        let f = super::failure(KernelError::caused(
            occt.clone(),
            Some("StdFail_NotDone".to_owned()),
        ));
        assert_eq!(
            (f.refusal, f.cause.as_deref()),
            (occt, Some("StdFail_NotDone"))
        );
    }

    use silico_protocol::{FailureKind, FreecadMeasures, ShapeSummary};

    use super::{check_finite, check_freecad_finite};

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
            assert_eq!(failure.kind(), FailureKind::KernelFailed);
            let said = failure.refusal.describe(None, &|e| e.to_string());
            assert_eq!(said.message, format!("the shape's {want}"));
        }
    }

    #[test]
    fn freecad_measures_that_arent_finite_are_refused() {
        let good = FreecadMeasures {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            bounds: Some([[0.0; 3], [10.0, 20.0, 30.0]]),
        };
        assert!(check_freecad_finite(&good).is_ok());
        let unbounded = FreecadMeasures {
            bounds: None,
            ..good.clone()
        };
        assert!(check_freecad_finite(&unbounded).is_ok());
        type Spoil = fn(&mut FreecadMeasures);
        let cases: [(&str, Spoil); 6] = [
            ("volume isn't finite (NaN)", |m| m.volume = f64::NAN),
            ("area isn't finite (inf)", |m| m.area = f64::INFINITY),
            ("center y isn't finite (NaN)", |m| m.center[1] = f64::NAN),
            ("center z isn't finite (-inf)", |m| {
                m.center[2] = f64::NEG_INFINITY;
            }),
            ("bounding box's lowest y isn't finite (NaN)", |m| {
                m.bounds = Some([[0.0, f64::NAN, 0.0], [10.0, 20.0, 30.0]]);
            }),
            ("bounding box's highest x isn't finite (inf)", |m| {
                m.bounds = Some([[0.0; 3], [f64::INFINITY, 20.0, 30.0]]);
            }),
        ];
        for (want, spoil) in cases {
            let mut bad = good.clone();
            spoil(&mut bad);
            let failure = check_freecad_finite(&bad).unwrap_err();
            assert_eq!(failure.kind(), FailureKind::KernelFailed);
            let said = failure.refusal.describe(None, &|e| e.to_string());
            assert_eq!(said.message, format!("the shape's {want}"));
        }
    }
}
