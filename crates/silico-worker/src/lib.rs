//! What every worker process is, whatever it links (ARCHITECTURE.md, Process model): a
//! process the core starts, which inherits one end of a socket pair as file descriptor
//! [`WORKER_FD`] and serves [`Request`]s on it until the core closes it, logging to stderr
//! as JSON lines. The kernel worker (OCCT) and the sketch worker (PlaneGCS) are each this
//! loop and a handler.
//!
//! A worker whose output is a result keeps floating-point traps off: the C++ it links
//! decides how they're read and switched off, so it passes them in as its [`Platform`]'s
//! [`Traps::Checked`]. A worker whose output is no result (the render worker's pixels) says
//! it doesn't check them, [`Traps::Unchecked`].

// Takes ownership of the inherited descriptor and closes the others: `unsafe` for those
// system calls only, the exception ARCHITECTURE.md's invariant 8 names (inherited
// descriptors, nothing else).
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

/// What a worker is to the loop: how its floating-point traps are kept off, and what it was
/// built from.
pub struct Platform {
    /// Whether, and how, the loop reads and switches off its floating-point traps.
    pub traps: Traps,
    /// Logged when the worker is ready, naming what it was built from (its C++ library).
    pub built_from: &'static str,
}

/// How a worker's floating-point traps are kept off (ARCHITECTURE.md, *Deterministic
/// kernel*, 7).
#[derive(Clone, Copy)]
pub enum Traps {
    /// Read and switched off through the C++ the worker links, at startup and after every
    /// request: the rule for every worker whose output is a result (a shape and its
    /// history, a solve, a measure, a mesh).
    Checked {
        /// The trap-enable bits that are set: 0 when every trap is off.
        mask: fn() -> u32,
        /// Switches every trap off.
        disable: fn() -> Result<(), String>,
    },
    /// Not read: a worker whose output is no result (the render worker's pixels), which
    /// *Deterministic kernel*'s rules don't bind. Logged at startup at info level (seen with
    /// `SILICO_LOG=info`; the default shows warnings), never stubbed as a mask of 0, which
    /// would say every trap is off rather than that none was looked at.
    Unchecked,
}

/// How the loop reads and switches traps: the only way it does, at startup and after every
/// request. Each is none where traps aren't checked.
impl Traps {
    /// The trap-enable bits that are set: 0 when every trap is off.
    pub fn mask(self) -> Option<u32> {
        match self {
            Traps::Checked { mask, .. } => Some(mask()),
            Traps::Unchecked => None,
        }
    }

    /// Fails if any floating-point trap is on.
    pub fn check(self) -> Option<Result<(), String>> {
        self.mask().map(check_fp_trap_mask)
    }

    /// Switches every trap off.
    pub fn disable(self) -> Option<Result<(), String>> {
        match self {
            Traps::Checked { disable, .. } => Some(disable()),
            Traps::Unchecked => None,
        }
    }
}

/// The decision behind [`Traps::check`], on a given trap-enable mask, so it can be
/// tested on every platform (some Apple silicon can't switch traps on to test it for real).
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
        let traps = platform.traps;
        if let Some(bits) = traps.mask()
            && let Err(problem) = check_fp_trap_mask(bits)
        {
            let fixed = traps.disable().is_some_and(|r| r.is_ok())
                && traps.check().is_some_and(|r| r.is_ok());
            tracing::error!("{problem} after {name}; switched off: {fixed}");
            result = Err(Failure::new(Refusal::FpTrapsOn {
                mask: bits,
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

/// The traps as a worker starts: switched off and checked off for [`Traps::Checked`] (or why
/// not, logged as an error, and the worker refuses to run), said to be unchecked for
/// [`Traps::Unchecked`].
fn traps_at_start(traps: Traps) -> Result<(), String> {
    match traps.disable() {
        None => {
            tracing::info!(
                traps = "unchecked",
                "floating-point traps aren't checked: no result is computed in this worker"
            );
            Ok(())
        }
        Some(Err(e)) => {
            let problem = format!("couldn't switch floating-point traps off: {e}");
            tracing::error!("{problem}");
            Err(problem)
        }
        Some(Ok(())) => match traps.check() {
            Some(Err(problem)) => {
                let problem = format!("{problem}; refusing to run");
                tracing::error!("{problem}");
                Err(problem)
            }
            _ => Ok(()),
        },
    }
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

    if traps_at_start(platform.traps).is_err() {
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use silico_protocol::{
        DebugOp, Failure, FrameReader, Generation, Op, Refusal, Reply, Request, RequestId,
        Response, TraceId, write_message,
    };

    use super::{Platform, Traps, serve, traps_at_start};

    /// What a test's subscriber writes.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Serves one request on `platform`, its handler answering `Done`: the response, and
    /// what the worker logged.
    fn serve_one(platform: &Platform) -> (Response, String) {
        let (core, worker) = UnixStream::pair().unwrap();
        let request = Request {
            id: RequestId(0),
            generation: Generation(0),
            trace: TraceId(1),
            op: Op::Debug(DebugOp::Sleep { millis: 0 }),
        };
        write_message(&mut &core, &request).unwrap();
        core.shutdown(std::net::Shutdown::Write).unwrap();
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            serve(worker, platform, |_, _, _, _| Ok(Reply::Done))
        })
        .unwrap();
        let mut frames = FrameReader::new(core);
        assert_eq!(
            frames.read_frame().unwrap(),
            Some(Vec::new()),
            "the started frame"
        );
        let response = frames.read_message::<Response>().unwrap().unwrap();
        let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        (response, log)
    }

    #[test]
    fn traps_still_on_after_switching_off_are_logged_as_not_switched_off() {
        // Switching off says it worked, but a trap bit stays set: they aren't off.
        let platform = Platform {
            traps: Traps::Checked {
                mask: || 0x100,
                disable: || Ok(()),
            },
            built_from: "test",
        };
        let (response, log) = serve_one(&platform);
        assert_eq!(
            response.result,
            Err(Failure::new(Refusal::FpTrapsOn {
                mask: 0x100,
                op: "DebugSleep".to_owned(),
            }))
        );
        assert!(log.contains("switched off: false"), "{log}");
    }

    #[test]
    fn traps_switched_back_off_are_logged_as_switched_off() {
        // On after the request and off once switched off: the request still fails, and the
        // log says they're off again (the other test's positive control).
        static ON: AtomicBool = AtomicBool::new(true);
        let platform = Platform {
            traps: Traps::Checked {
                mask: || u32::from(ON.load(Ordering::SeqCst)),
                disable: || {
                    ON.store(false, Ordering::SeqCst);
                    Ok(())
                },
            },
            built_from: "test",
        };
        let (response, log) = serve_one(&platform);
        assert!(
            matches!(
                response.result,
                Err(Failure {
                    refusal: Refusal::FpTrapsOn { mask: 1, .. },
                    ..
                })
            ),
            "{:?}",
            response.result
        );
        assert!(log.contains("switched off: true"), "{log}");
    }

    #[test]
    fn an_unchecked_platform_serves_without_reading_traps() {
        // A worker whose output is no result (the render worker): its traps aren't read, so
        // a request is answered as handled, never refused as traps on, and nothing about
        // traps is logged. The checked tests above are its control: the same request with a
        // mask set is refused.
        let platform = Platform {
            traps: Traps::Unchecked,
            built_from: "test",
        };
        let (response, log) = serve_one(&platform);
        assert_eq!(response.result, Ok(Reply::Done));
        assert!(!log.contains("traps"), "{log}");
        assert_eq!(Traps::Unchecked.check(), None);
        let checked = Traps::Checked {
            mask: || 0x100,
            disable: || Ok(()),
        };
        assert!(checked.check().is_some_and(|r| r.is_err()));
    }
    /// `traps_at_start` on `traps`: what it returned, and what it logged at info and above.
    fn start_with(traps: Traps) -> (Result<(), String>, String) {
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        let started = tracing::subscriber::with_default(subscriber, || traps_at_start(traps));
        let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        (started, log)
    }

    #[test]
    fn a_worker_starts_with_its_traps_off_or_says_they_are_unchecked_or_refuses_to_run() {
        // Unchecked: it runs, and says so once.
        let (started, log) = start_with(Traps::Unchecked);
        assert_eq!(started, Ok(()));
        assert!(log.contains("traps=\"unchecked\""), "{log}");
        // Checked, off: it runs, saying nothing about traps.
        let (started, log) = start_with(Traps::Checked {
            mask: || 0,
            disable: || Ok(()),
        });
        assert_eq!(started, Ok(()));
        assert!(!log.contains("traps"), "{log}");
        // Checked, but they can't be switched off: it refuses to run.
        let (started, log) = start_with(Traps::Checked {
            mask: || 0,
            disable: || Err("no".to_owned()),
        });
        assert!(started.unwrap_err().contains("couldn't switch"));
        assert!(
            log.contains("couldn't switch floating-point traps off: no"),
            "{log}"
        );
        // Checked, switched off, but still on: it refuses to run.
        let (started, log) = start_with(Traps::Checked {
            mask: || 0x100,
            disable: || Ok(()),
        });
        assert!(started.unwrap_err().contains("refusing to run"));
        assert!(log.contains("trap-enable bits 0x100"), "{log}");
    }
}
