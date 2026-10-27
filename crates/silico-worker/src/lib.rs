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
    Failure, FrameError, FrameReader, Generation, MAX_FRAME_LEN, Op, REQUEST_DONE, Refusal, Reply,
    Request, RequestId, Response, WORKER_FD, WORKER_FD_ENV, write_message_within, write_started,
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
    /// *Deterministic kernel*'s rules don't bind. Logged at startup at info level (seen in
    /// the app's log with `SILICO_LOG=info`; the default shows warnings), never stubbed as a
    /// mask of 0, which would say every trap is off rather than that none was looked at.
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

/// How much of a worker's line a refusal's message may take, written as the line's JSON
/// escapes it: half of the line the core reads ([`silico_protocol::MAX_LOG_LINE`]), the rest
/// room for its time, level, target, and spans.
const REFUSAL_LINE_ROOM: usize = silico_protocol::MAX_LOG_LINE / 2;

/// `text`, or as much of it as fits in `room` bytes once JSON escapes it (a quote or a
/// backslash two bytes, a control character two or six), cut at a character's boundary and
/// ending in `…`.
fn within_line(text: &str, room: usize) -> std::borrow::Cow<'_, str> {
    const CUT: char = '…';
    let escaped = |c: char| match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if c < ' ' => 6,
        c => c.len_utf8(),
    };
    if text.chars().map(escaped).sum::<usize>() <= room {
        return std::borrow::Cow::Borrowed(text);
    }
    let room = room.saturating_sub(CUT.len_utf8());
    let mut used = 0;
    let mut end = 0;
    for (at, c) in text.char_indices() {
        used += escaped(c);
        if used > room {
            break;
        }
        end = at + c.len_utf8();
    }
    let mut cut = text[..end].to_owned();
    cut.push(CUT);
    std::borrow::Cow::Owned(cut)
}

/// A worker's log filter, from the `SILICO_LOG` it inherits: its directives, each one less
/// than info (`warn`, `silico=error`, `[step]=off`) raised to info, and info for every target
/// none names, so a worker always writes its info lines, whatever the app's log shows. The
/// core filters what it re-emits to the app's log itself, by the same `SILICO_LOG`, and
/// keeps each session's events at every level for `read_logs` (silico_core::command::logs),
/// where a request's refusal, in the worker's words, is an info line. Unset, empty, or a
/// filter it can't use (which the app reports, silico_core::logging), it's info.
fn worker_filter(value: Option<&str>) -> tracing_subscriber::EnvFilter {
    use tracing::level_filters::LevelFilter;
    let level = |text: &str| text.trim().parse::<LevelFilter>().ok();
    let raised = value.filter(|value| !value.trim().is_empty()).map(|value| {
        let mut every_target = false;
        // An empty one is none, as the app's filter reads it (`debug,`): never a level.
        let mut directives: Vec<String> = value
            .split(',')
            .filter(|directive| !directive.trim().is_empty())
            .map(|directive| match directive.rsplit_once('=') {
                Some((what, at)) if level(at).is_some_and(|at| at < LevelFilter::INFO) => {
                    format!("{what}=info")
                }
                Some(_) => directive.to_owned(),
                None => match level(directive) {
                    Some(at) => {
                        every_target = true;
                        if at < LevelFilter::INFO {
                            "info".to_owned()
                        } else {
                            directive.to_owned()
                        }
                    }
                    None => directive.to_owned(),
                },
            })
            .collect();
        if !every_target {
            directives.push("info".to_owned());
        }
        directives.join(",")
    });
    raised
        .and_then(|value| tracing_subscriber::EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new("info"))
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
        // In the request's span, so the core files it under the call that asked: a
        // session's logs hold the worker's own line for what it refused, its kind and OCCT's
        // words. Not its elements: the worker knows them only by their places in its shapes,
        // which aren't the document's names, and the core's line for the refusal names
        // them (ARCHITECTURE.md, invariant 2: the core words a refusal).
        if let Err(failure) = &result {
            let kind = failure.refusal.kind().word();
            // Its cause in the message: the core re-emits a worker's line by its message. Cut
            // to fit the line the core reads: OCCT's words can be long.
            let message = match &failure.cause {
                Some(cause) => format!("{name} refused, {kind}: {cause}"),
                None => format!("{name} refused, {kind}"),
            };
            tracing::info!("{}", within_line(&message, REFUSAL_LINE_ROOM));
        }
        // The request's last log line, after any it logged and before its reply: the core
        // keeps the span the request was asked in until it has read this, so every line of
        // the request goes under it, and then lets it go.
        {
            let mut log = std::io::stderr().lock();
            let _ = writeln!(log, "{{\"{REQUEST_DONE}\":{}}}", request.trace.0);
        }
        reply(
            &mut writer,
            request.id,
            request.generation,
            result,
            MAX_FRAME_LEN,
        )?;
    }
    Ok(())
}

/// Writes the reply `result` to the request `id` of `generation`; or, if it's past the `most`
/// bytes a frame carries, a failure in its place saying so ([`Refusal::ReplyTooLarge`]), so
/// the request is refused and the worker goes on, rather than ending as if it had crashed
/// and being started again for no fault of its own.
fn reply(
    writer: &mut dyn Write,
    id: RequestId,
    generation: Generation,
    result: Result<Reply, Failure>,
    most: u32,
) -> Result<(), FrameError> {
    let response = Response {
        id,
        generation,
        result,
    };
    match write_message_within(writer, &response, most) {
        Err(FrameError::TooLarge { len, max }) => {
            // Past the request's last log line, so it's the core that logs the refusal.
            let refusal = Refusal::ReplyTooLarge {
                bytes: len,
                most: u64::from(max),
            };
            write_message_within(
                writer,
                &Response {
                    id,
                    generation,
                    result: Err(Failure::new(refusal)),
                },
                most,
            )
        }
        written => written,
    }
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
    let filter = worker_filter(std::env::var("SILICO_LOG").ok().as_deref());
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
        Blob, DebugOp, Failure, FrameReader, Generation, Op, Refusal, Reply, Request, RequestId,
        Response, TraceId, write_message,
    };

    use super::{
        Platform, REFUSAL_LINE_ROOM, Traps, serve, traps_at_start, within_line, worker_filter,
    };

    #[test]
    fn a_reply_past_what_a_frame_carries_is_refused_in_its_place_and_the_worker_goes_on() {
        let refused = |bytes, most| Response {
            id: RequestId(3),
            generation: Generation(2),
            result: Err(Failure::new(Refusal::ReplyTooLarge { bytes, most })),
        };
        // A response's size encoded: its frame less the frame's 4-byte length.
        let encoded = |response: &Response| {
            let mut frame = Vec::new();
            write_message(&mut frame, response).unwrap();
            frame.len() - 4
        };
        // The refusal's own size is the frame's limit here: an echo of more than that can't
        // be sent, and the refusal in its place can.
        let most = encoded(&refused(1 << 20, 1 << 20));
        let echo = |n: usize| {
            Ok(Reply::Echo {
                data: Blob::inline(vec![7; n]),
            })
        };
        let echoed = |n: usize| {
            encoded(&Response {
                id: RequestId(3),
                generation: Generation(2),
                result: echo(n),
            })
        };
        let past = (0..).find(|&n| echoed(n) > most).unwrap();
        let mut written = Vec::new();
        super::reply(
            &mut written,
            RequestId(3),
            Generation(2),
            echo(past),
            u32::try_from(most).unwrap(),
        )
        .unwrap();
        let mut frames = FrameReader::new(&written[..]);
        let sent = frames.read_message::<Response>().unwrap().unwrap();
        assert_eq!(
            sent,
            refused(echoed(past) as u64, most as u64),
            "the refusal names the reply's size and the limit"
        );
        assert_eq!(
            frames.read_frame().unwrap(),
            None,
            "one frame, nothing after"
        );
        // One byte less: the echo itself, as it was.
        let mut written = Vec::new();
        super::reply(
            &mut written,
            RequestId(3),
            Generation(2),
            echo(past - 1),
            u32::try_from(most).unwrap(),
        )
        .unwrap();
        let sent = FrameReader::new(&written[..])
            .read_message::<Response>()
            .unwrap()
            .unwrap();
        assert_eq!(sent.result, echo(past - 1));
    }

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
    fn a_refusal_is_logged_at_info_in_its_requests_span_its_line_within_what_the_core_reads() {
        // A refusal whose words grow with its request (OCCT's, here), in characters
        // JSON escapes: its line, as the worker's JSON subscriber writes it, still fits.
        let (core, worker) = UnixStream::pair().unwrap();
        let request = Request {
            id: RequestId(0),
            generation: Generation(0),
            trace: TraceId(7),
            op: Op::Debug(DebugOp::Sleep { millis: 0 }),
        };
        write_message(&mut &core, &request).unwrap();
        core.shutdown(std::net::Shutdown::Write).unwrap();
        // Its reply, which names the op too, read as it's written, so the worker never
        // waits on a full socket.
        let replies = std::thread::spawn(move || {
            let mut frames = FrameReader::new(core);
            frames.read_frame().unwrap();
            frames.read_message::<Response>().unwrap().unwrap()
        });
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(move || writer.clone())
            .finish();
        // OCCT's words, which the line carries whole when they fit.
        let cause = "\"\u{1}é\\".repeat(3000);
        let platform = Platform {
            traps: Traps::Unchecked,
            built_from: "test",
        };
        tracing::subscriber::with_default(subscriber, || {
            serve(worker, &platform, |_, _, _, _| {
                Err(Failure {
                    refusal: Refusal::OtherWorkersOp { op: "x".to_owned() },
                    cause: Some(cause.clone()),
                })
            })
        })
        .unwrap();
        assert!(replies.join().unwrap().result.is_err());
        let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let line = log
            .lines()
            .find(|line| line.contains("DebugSleep refused, "))
            .unwrap_or_else(|| panic!("{log}"));
        assert!(
            line.len() <= silico_protocol::MAX_LOG_LINE,
            "{}",
            line.len()
        );
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["span"]["trace"], 7);
        let message = event["fields"]["message"].as_str().unwrap();
        assert!(message.ends_with('…'), "{message}");
        assert!(
            message.starts_with("DebugSleep refused, unknown_op: \"\u{1}é"),
            "{message}"
        );
    }

    #[test]
    fn a_line_is_cut_to_fit_as_json_escapes_it_at_a_characters_boundary() {
        // Each cut checked against serde_json's own escaping, which the worker's JSON
        // subscriber writes with.
        let escaped = |text: &str| serde_json::to_string(text).unwrap().len() - 2;
        for text in [
            "plain",
            // A space is one byte, as written: the first character not escaped.
            "a b",
            "\"quoted\" and \\ back",
            "\u{1}\u{1f}\n\t\r\u{8}\u{c}",
            "é🦀\u{7f}",
            &"\"".repeat(100),
            &"\u{1}".repeat(100),
            &"🦀".repeat(100),
        ] {
            for room in [0, 1, 2, 3, 4, 5, 6, 7, 10, 64, 1000] {
                let cut = within_line(text, room);
                if escaped(text) <= room {
                    assert_eq!(cut, text, "{text:?} in {room}");
                    continue;
                }
                let kept = cut.strip_suffix('…').unwrap();
                assert!(text.starts_with(kept), "{text:?} in {room}: {cut:?}");
                assert!(escaped(&cut) <= room.max(3), "{text:?} in {room}: {cut:?}");
                // As much as fits: one character more wouldn't have.
                let more = text[kept.len()..].chars().next().unwrap();
                assert!(
                    escaped(kept) + escaped(&more.to_string()) + 3 > room,
                    "{text:?} in {room}: {cut:?}"
                );
            }
        }
        const { assert!(REFUSAL_LINE_ROOM * 2 <= silico_protocol::MAX_LOG_LINE) };
    }

    #[test]
    fn a_worker_writes_info_whatever_level_silico_log_gives_every_target() {
        use tracing::level_filters::LevelFilter;
        let hint = |value: Option<&str>| worker_filter(value).max_level_hint();
        for value in [
            None,
            Some(""),
            Some(" "),
            Some("warn"),
            Some("WARN"),
            Some("error"),
            Some("off"),
            Some("info"),
            Some("silico=loud"),
        ] {
            assert_eq!(hint(value), Some(LevelFilter::INFO), "{value:?}");
        }
        // Targets or spans alone (robustness's proof, round 1): every target none names is
        // at info, and one named below info is raised to it.
        for value in [
            "silico_core=debug",
            "silico=warn",
            "[recompute_step]=debug",
            "silico_worker=off",
        ] {
            let log = Log::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_env_filter(worker_filter(Some(value)))
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(target: "silico_worker", "DebugSleep refused: x");
                tracing::debug!(target: "silico_worker", "not this");
            });
            let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert!(
                log.contains("refused"),
                "SILICO_LOG={value}: no info line: {log}"
            );
            assert!(!log.contains("not this"), "SILICO_LOG={value}: {log}");
        }
        assert_eq!(
            worker_filter(Some("silico=warn")).to_string(),
            "silico=info,info"
        );
        assert_eq!(hint(Some("silico_core=debug")), Some(LevelFilter::DEBUG));
        // A level more verbose than info, as given: the lines written, not the filter's hint,
        // which a directive overwritten by a later one keeps (tests round 2's proof), and an
        // empty directive is none (robustness round 2's: `debug,` read `debug,info`).
        for value in [
            "debug",
            "warn,debug",
            "debug,",
            ",debug",
            "debug, ",
            " ,debug",
            "silico_worker=debug",
        ] {
            let log = Log::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_env_filter(worker_filter(Some(value)))
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                tracing::debug!(target: "silico_worker", "a debug line");
                tracing::trace!(target: "silico_worker", "not this");
            });
            let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert!(log.contains("a debug line"), "SILICO_LOG={value}: {log}");
            assert!(!log.contains("not this"), "SILICO_LOG={value}: {log}");
        }
        assert_eq!(worker_filter(Some("debug,")).to_string(), "debug");
        assert_eq!(worker_filter(Some("debug, ")).to_string(), "debug");
        // More than info, or a target's own, as given.
        assert_eq!(hint(Some("debug")), Some(LevelFilter::DEBUG));
        assert_eq!(
            hint(Some("warn,silico_kernel=trace")),
            Some(LevelFilter::TRACE)
        );
        assert_eq!(
            worker_filter(Some("warn,silico_kernel=trace")).to_string(),
            "silico_kernel=trace,info"
        );
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
        // Unchecked: it runs, and says so once, at info (unseen at the default warn level).
        let (started, log) = start_with(Traps::Unchecked);
        assert_eq!(started, Ok(()));
        assert_eq!(log.matches("traps=\"unchecked\"").count(), 1, "{log}");
        assert!(log.contains(" INFO "), "{log}");
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
        assert!(log.contains("ERROR"), "{log}");
        // Checked, switched off, but still on: it refuses to run.
        let (started, log) = start_with(Traps::Checked {
            mask: || 0x100,
            disable: || Ok(()),
        });
        assert!(started.unwrap_err().contains("refusing to run"));
        assert!(log.contains("trap-enable bits 0x100"), "{log}");
        assert!(log.contains("ERROR"), "{log}");
    }

    #[test]
    fn traps_on_at_start_are_switched_off_and_the_worker_runs() {
        // The council's tests lens: switching off comes before the check, so traps a linked
        // library left on are switched off, and the worker runs (checked first, it would
        // refuse to start with them on).
        static ON: AtomicBool = AtomicBool::new(true);
        let (started, log) = start_with(Traps::Checked {
            mask: || u32::from(ON.load(Ordering::SeqCst)),
            disable: || {
                ON.store(false, Ordering::SeqCst);
                Ok(())
            },
        });
        assert_eq!(started, Ok(()), "{log}");
        assert!(!ON.load(Ordering::SeqCst));
    }
}
