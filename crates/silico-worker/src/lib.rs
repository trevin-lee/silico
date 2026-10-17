//! What every worker process is, whatever it links (ARCHITECTURE.md, Process model): a
//! process the core starts, which inherits one end of a socket pair as file descriptor
//! [`WORKER_FD`] and serves [`Request`]s on it until the core closes it, logging to stderr
//! as JSON lines, with floating-point traps off. The kernel worker (OCCT) and the sketch
//! worker (PlaneGCS) are each this loop and a handler.
//!
//! The C++ each worker links decides how floating-point traps are read and switched off,
//! so a worker passes them in as a [`Platform`].

// Takes ownership of the inherited descriptor and closes the others: `unsafe` for those
// system calls only (ARCHITECTURE.md, invariant 8: a crate at the OS boundary).
#![allow(unsafe_code)]

use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use silico_protocol::{
    Failure, FrameError, FrameReader, Generation, Op, REQUEST_DONE, Refusal, Reply, Request,
    RequestId, Response, WORKER_FD, WORKER_FD_ENV, write_message, write_started,
};

#[cfg(feature = "test_ops")]
pub mod test_ops;

/// How a worker reads and switches off floating-point traps, through the C++ it links.
pub struct Platform {
    /// The trap-enable bits that are set: 0 when every trap is off.
    pub fp_trap_mask: fn() -> u32,
    /// Switches every trap off.
    pub disable_fp_traps: fn() -> Result<(), String>,
    /// Logged when the worker is ready, naming what it was built from (its C++ library).
    pub built_from: &'static str,
}

/// Fails if any floating-point trap is on. Checked at startup and after every request.
pub fn check_fp_traps(platform: &Platform) -> Result<(), String> {
    check_fp_trap_mask((platform.fp_trap_mask)())
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
/// (the test workers' garbage and out-of-turn operations do). Returns an error if the
/// stream breaks or a request can't be read.
pub fn serve<F>(socket: UnixStream, platform: &Platform, mut handle: F) -> Result<(), FrameError>
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
        let mask = (platform.fp_trap_mask)();
        if let Err(problem) = check_fp_traps(platform) {
            let fixed = (platform.disable_fp_traps)().is_ok() && check_fp_traps(platform).is_ok();
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
pub fn run<F>(platform: &Platform, handle: F) -> ExitCode
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

    if let Err(e) = (platform.disable_fp_traps)() {
        tracing::error!("couldn't switch floating-point traps off: {e}");
        return ExitCode::from(3);
    }
    if let Err(problem) = check_fp_traps(platform) {
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
    tracing::debug!(built_from = platform.built_from, "worker ready");
    match serve(socket, platform, handle) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("stopping: {e}");
            ExitCode::FAILURE
        }
    }
}
