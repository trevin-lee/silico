//! Silico's MCP server (docs/design/m4.9-mcp.md): the core's commands as MCP tools, over
//! JSON-RPC 2.0, one message a line, revision 2025-06-18. A client of the core's command
//! API like the CLI and the app: every tool is a command run through
//! [`Sessions::call`](silico_core::command::Sessions::call), whose envelope is the tool's
//! answer, byte for byte.
//!
//! [`Server::new`] takes the sessions (in their workspace, as `silico serve` makes them);
//! [`Server::serve`] reads requests from its input and writes replies to its output until
//! the input ends, a write fails (the client closed its end), or [`Stop::stop`] is called
//! (a signal): then it cancels every call in flight and drops the sessions, which rolls
//! back each open transaction, and returns how it ended.
//!
//! **How it runs.** A reader thread reads lines, each at most [`MAX_LINE`] bytes (a
//! longer one skipped to its end, never held past that), and hands each to `serve`'s own
//! thread, which handles them in line order: it answers the handshake, `ping`, and
//! `tools/list` itself, and starts each `tools/call` through `Sessions::call`, which queues
//! the command and returns at once, so commands reach a session in the order the client
//! sent them. Because it reads (parses) each call's arguments to queue them, the time it
//! takes to get to the next line grows with a request's size; that's by design, so a
//! session's commands are queued in line order, and isn't to be "fixed" by queuing
//! unparsed text. A call's answer is waited for on a thread of its own, which sends
//! progress if the call asked for it and writes the answer: at most [`MAX_IN_FLIGHT`] such
//! calls run at once, a call past that answered at once with a refusal. `status`,
//! `cancel`, `version`, and `schema`, which never wait on a session, take no slot: they're
//! answered in turn on one thread of the server's, with its other replies, so a `cancel`
//! is answered even with every slot held. Every message is written whole under one lock
//! on the output; whether a call is answered or cancelled is decided under a lock of its
//! own, never the output's, so the reader never waits on a slow client's output but when
//! the replies it has queued pass [`MAX_QUEUED_REPLIES`] (backpressure, never an unbounded
//! queue).

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use silico_core::command::{Answered, Call, Calling, Command, Problems, Sessions, Stopper};
use silico_core::kernel::Clients;
use silico_core::{Error, ErrorCode};

mod frame;
mod reply;
pub mod tools;

pub use frame::{FRAME_ROOM, MAX_LINE};

use frame::{Bad, Id, Line, Message};
use reply::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, Output, PARSE_ERROR};

/// The MCP revision spoken, whatever the client asks for.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The most calls that wait on a session at once (the maintainer's choice, 2026-10-26: a
/// transport limit, the server's, not the core's).
pub const MAX_IN_FLIGHT: usize = 64;

/// The most replies the reader queues for the output before it waits for the client to
/// read them.
pub const MAX_QUEUED_REPLIES: usize = 64;

/// How often a call that asked for progress looks at it.
pub const PROGRESS_STEP: Duration = Duration::from_millis(100);

/// The longest a name from the client is quoted in an error.
const QUOTED_AT_MOST: usize = 200;

/// The methods answered.
const METHODS: [&str; 4] = ["initialize", "ping", "tools/list", "tools/call"];

/// How [`Server::serve`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The input ended: the client closed its end.
    Input,
    /// Reading the input failed.
    InputFailed(String),
    /// A write failed: the client closed its end.
    Output,
    /// [`Stop::stop`] was called.
    Stopped,
}

/// The MCP server: the sessions it runs commands on, and what it answers with.
pub struct Server<P: Clients + 'static> {
    sessions: Sessions<P>,
    /// `tools/list`'s result, on one line.
    tools: String,
    /// `initialize`'s result, on one line.
    initialized: String,
    control: Arc<Control>,
    events: Receiver<Event>,
}

/// What ends [`Server::serve`] from another thread: a signal, while the reader is blocked
/// on its input.
#[derive(Clone)]
pub struct Stop {
    control: Arc<Control>,
}

impl Stop {
    /// Ends `serve` as the input's end does: every call in flight cancelled, the sessions
    /// dropped.
    pub fn stop(&self) {
        self.control.end(Ended::Stopped, Event::Stop);
    }
}

/// What the reader, the output, and [`Stop`] tell `serve`.
enum Event {
    Line(Line),
    Stop,
    OutputFailed,
    /// The replies' thread has written every reply queued, and ended.
    Drained,
}

/// How `serve` is told to end, shared by everything that can end it.
pub(crate) struct Control {
    ending: Mutex<Option<Ended>>,
    soon: AtomicBool,
    events: Sender<Event>,
}

impl Control {
    fn end(&self, why: Ended, event: Event) {
        lock(&self.ending).get_or_insert(why);
        self.soon.store(true, Ordering::SeqCst);
        let _ = self.events.send(event);
    }

    /// A write to the client failed.
    pub(crate) fn output_failed(&self) {
        self.end(Ended::Output, Event::OutputFailed);
    }

    fn ending(&self) -> bool {
        self.soon.load(Ordering::SeqCst)
    }

    fn why(&self) -> Option<Ended> {
        lock(&self.ending).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<P: Clients + 'static> Server<P> {
    /// The server of `sessions`: its tool list built from the core's snapshots, refused if
    /// one can't be (a tool's output schema, say, whose definitions clash: Silico's bug).
    pub fn new(sessions: Sessions<P>) -> Result<Self, Error> {
        let list = tools::list()?;
        let tools = serde_json::to_string(&list).map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!("the tool list couldn't be written: {e}"),
            )
        })?;
        let workspace = sessions.workspace().map(|w| w.root().display().to_string());
        let initialized = serde_json::to_string(&json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "silico", "title": "Silico", "version": env!("CARGO_PKG_VERSION")},
            "instructions": instructions(workspace.as_deref()),
        }))
        .map_err(|e| {
            Error::new(
                ErrorCode::Internal,
                format!("initialize's answer couldn't be written: {e}"),
            )
        })?;
        let (sender, events) = mpsc::channel();
        Ok(Server {
            sessions,
            tools,
            initialized,
            control: Arc::new(Control {
                ending: Mutex::new(None),
                soon: AtomicBool::new(false),
                events: sender,
            }),
            events,
        })
    }

    /// What ends [`serve`](Self::serve) from another thread.
    pub fn stopper(&self) -> Stop {
        Stop {
            control: Arc::clone(&self.control),
        }
    }

    /// Serves one client: requests read from `input`, replies written to `output`, until
    /// the input ends, a write fails, or [`Stop::stop`]; then every call in flight is
    /// cancelled and the sessions dropped. Reading is done on a thread of its own, which a
    /// blocked read keeps after this returns (until the process ends, or the read does).
    pub fn serve<R, W>(self, input: R, output: W) -> Ended
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let _span = tracing::info_span!("serve").entered();
        let Server {
            sessions,
            tools,
            initialized,
            control,
            events,
        } = self;
        let output = Arc::new(Output::new(Box::new(output), Arc::clone(&control)));
        let (credit, credits) = mpsc::sync_channel::<()>(1);
        let reader = {
            let control = Arc::clone(&control);
            std::thread::Builder::new()
                .name("mcp-reader".to_owned())
                .spawn(move || read(input, &control.events, &credits))
        };
        if let Err(e) = reader {
            tracing::error!("couldn't start the reader's thread: {e}");
            return Ended::InputFailed(format!("couldn't start the reader's thread: {e}"));
        }
        let (lane, jobs) = mpsc::sync_channel::<Job>(MAX_QUEUED_REPLIES);
        let replier = {
            let output = Arc::clone(&output);
            let control = Arc::clone(&control);
            std::thread::Builder::new()
                .name("mcp-replies".to_owned())
                .spawn(move || {
                    for job in jobs {
                        match job {
                            Job::Text(text) => {
                                output.send(reply::text(&text));
                            }
                            Job::Call(waiter, calling) => waiter.run(calling),
                        }
                    }
                    let _ = control.events.send(Event::Drained);
                })
        };
        if let Err(e) = replier {
            tracing::error!("couldn't start the replies' thread: {e}");
            return Ended::InputFailed(format!("couldn't start the replies' thread: {e}"));
        }
        let mut dispatch = Dispatch {
            sessions: &sessions,
            tools,
            initialized_text: initialized,
            initialized: false,
            flights: Arc::new(Flights::default()),
            output,
            lane,
            control: Arc::clone(&control),
        };
        let ended = loop {
            if let Some(why) = control.why() {
                break why;
            }
            match events.recv() {
                Ok(Event::Line(Line::End)) => break Ended::Input,
                Ok(Event::Line(Line::Failed(e))) => break Ended::InputFailed(e.to_string()),
                Ok(Event::Line(Line::Whole(line))) => {
                    dispatch.message(&line);
                    let _ = credit.send(());
                }
                Ok(Event::Line(Line::TooLong { head, length })) => {
                    dispatch.too_long(&head, length);
                    let _ = credit.send(());
                }
                Ok(Event::Stop) => break Ended::Stopped,
                Ok(Event::OutputFailed) => break Ended::Output,
                // Never: the replies' thread ends only once `dispatch` is dropped, and the
                // control holds a sender.
                Ok(Event::Drained) | Err(_) => break Ended::Input,
            }
        };
        tracing::info!(?ended, "the client went away: cancelling its calls");
        dispatch.flights.cancel_all();
        drop(dispatch);
        // A client that closed its input may still read what it was answered before: the
        // replies queued are written, unless it's stopped or its output fails meanwhile.
        if matches!(ended, Ended::Input | Ended::InputFailed(_)) {
            while let Ok(Event::Line(_)) = events.recv() {}
        }
        drop(sessions);
        ended
    }
}

/// `initialize`'s instructions: the workspace, and the three things an agent most needs.
fn instructions(workspace: Option<&str>) -> String {
    let paths = match workspace {
        Some(root) => format!(
            "The workspace is {root}: every path a tool takes (open's path, render's to) is relative to it, and none may lead out of it."
        ),
        None => "Paths are taken as the system resolves them.".to_owned(),
    };
    format!(
        "Silico is a parametric CAD system: open a document (a .silico file) as a session, edit its features, and read its model's elements and measures. {paths} {} {} {}",
        tools::IN_A_TRANSACTION,
        tools::READ_CURRENT,
        tools::NAMES_FROM_ELEMENTS
    )
}

/// The reader: each line of `input` sent on, the next read once the last is taken.
fn read(mut input: impl BufRead, events: &Sender<Event>, credits: &Receiver<()>) {
    loop {
        let line = frame::read_line(&mut input, frame::MAX_LINE);
        let last = matches!(line, Line::End | Line::Failed(_));
        if events.send(Event::Line(line)).is_err() || last || credits.recv().is_err() {
            return;
        }
    }
}

/// What the replies' thread does, in turn.
enum Job {
    /// Writes a reply already made.
    Text(String),
    /// Answers a call that waits on no session.
    Call(Waiter, Calling),
}

/// The calls in flight: by their IDs, those not yet answered or cancelled; and how many
/// hold a slot, until their waiters end.
#[derive(Default)]
struct Flights {
    table: Mutex<Table>,
}

#[derive(Default)]
struct Table {
    calls: BTreeMap<Id, Arc<Flight>>,
    slotted: usize,
}

/// A call in flight.
struct Flight {
    fate: Mutex<Fate>,
    stopper: Mutex<Option<Stopper>>,
    slotted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Running,
    Answered,
    Cancelled,
}

impl Flights {
    fn contains(&self, id: &Id) -> bool {
        lock(&self.table).calls.contains_key(id)
    }

    /// A new call `id`, holding a slot if `slotted`: none if every slot is held.
    fn take(self: &Arc<Self>, id: &Id, slotted: bool) -> Option<Landing> {
        let mut table = lock(&self.table);
        if slotted {
            if table.slotted >= MAX_IN_FLIGHT {
                return None;
            }
            table.slotted += 1;
        }
        let flight = Arc::new(Flight {
            fate: Mutex::new(Fate::Running),
            stopper: Mutex::new(None),
            slotted,
        });
        table.calls.insert(id.clone(), Arc::clone(&flight));
        Some(Landing {
            flights: Arc::clone(self),
            id: id.clone(),
            flight,
        })
    }

    /// `flight`, if it's still the call `id`, no longer in flight by that ID.
    fn forget(&self, id: &Id, flight: &Arc<Flight>) {
        let mut table = lock(&self.table);
        if table.calls.get(id).is_some_and(|f| Arc::ptr_eq(f, flight)) {
            table.calls.remove(id);
        }
    }

    /// Cancels the call `id`, if it's in flight and not answered (MCP's cancellation): it
    /// gets no reply. Whether it was.
    fn cancel(&self, id: &Id) -> bool {
        let flight = lock(&self.table).calls.get(id).cloned();
        match flight {
            Some(flight) => {
                let cancelled = flight.cancel();
                self.forget(id, &flight);
                cancelled
            }
            None => false,
        }
    }

    /// Cancels every call in flight.
    fn cancel_all(&self) {
        let calls = std::mem::take(&mut lock(&self.table).calls);
        for flight in calls.values() {
            flight.cancel();
        }
    }
}

impl Flight {
    /// Marks it cancelled and stops it, unless it's answered: whether it was cancelled.
    fn cancel(&self) -> bool {
        {
            let mut fate = lock(&self.fate);
            if *fate != Fate::Running {
                return false;
            }
            *fate = Fate::Cancelled;
        }
        if let Some(stopper) = lock(&self.stopper).as_ref() {
            stopper.cancel();
        }
        true
    }

    /// Marks it answered, unless it's cancelled: whether its answer is to be written.
    fn answer(&self) -> bool {
        let mut fate = lock(&self.fate);
        if *fate != Fate::Running {
            return false;
        }
        *fate = Fate::Answered;
        true
    }

    fn cancelled(&self) -> bool {
        *lock(&self.fate) == Fate::Cancelled
    }
}

/// A call's place in flight, given up when it's dropped (its waiter ends): its slot, and
/// its ID if it's still in flight.
struct Landing {
    flights: Arc<Flights>,
    id: Id,
    flight: Arc<Flight>,
}

impl Drop for Landing {
    fn drop(&mut self) {
        let mut table = lock(&self.flights.table);
        if table
            .calls
            .get(&self.id)
            .is_some_and(|f| Arc::ptr_eq(f, &self.flight))
        {
            table.calls.remove(&self.id);
        }
        if self.flight.slotted {
            table.slotted = table.slotted.saturating_sub(1);
        }
    }
}

/// What waits for a call's answer and writes it.
struct Waiter {
    id: Id,
    landing: Landing,
    /// The call's progress token, as the client wrote it, if it asked for progress.
    token: Option<String>,
    output: Arc<Output>,
}

impl Waiter {
    fn run(self, calling: Calling) {
        let _span = tracing::info_span!("tools/call", id = self.id.raw()).entered();
        let answered = match &self.token {
            None => calling.wait(),
            Some(token) => self.with_progress(calling, token),
        };
        let flight = &self.landing.flight;
        if flight.answer() {
            // No longer in flight by its ID before its answer is written, so the client
            // may give the ID again as soon as it reads it.
            self.landing.flights.forget(&self.id, flight);
            self.output.send(reply::answer(&self.id, &answered));
        } else {
            tracing::debug!("cancelled: its answer isn't sent");
        }
    }

    /// The call's answer, waited for in steps, its progress sent between: a count that
    /// grows by each step seen done and never falls (the session's newest rebuild may be
    /// another than the call's own, whose `done` can fall), with the step running as its
    /// message, sent only when it grows and the output is free.
    fn with_progress(&self, mut calling: Calling, token: &str) -> Answered {
        let (mut count, mut sent, mut last) = (0u64, 0u64, 0usize);
        loop {
            match calling.wait_timeout(PROGRESS_STEP) {
                Ok(answered) => return answered,
                Err(still) => calling = still,
            }
            let Some(progress) = calling.progress() else {
                continue;
            };
            count = grown(count, last, progress.done);
            last = progress.done;
            if count > sent
                && !self.landing.flight.cancelled()
                && self.output.send_if_free(reply::progress(
                    token,
                    count,
                    progress.running.as_deref(),
                ))
            {
                sent = count;
            }
        }
    }
}

/// A call's progress `count`, its newest rebuild's `done` steps having been `last` and now
/// `done`: grown by each step done since, never fallen (a `done` that falls is another
/// rebuild's, begun since, whose steps are counted from it).
fn grown(count: u64, last: usize, done: usize) -> u64 {
    count.saturating_add(done.saturating_sub(last) as u64)
}

/// `serve`'s own thread's work: each message handled in line order.
struct Dispatch<'s, P: Clients + 'static> {
    sessions: &'s Sessions<P>,
    tools: String,
    initialized_text: String,
    initialized: bool,
    flights: Arc<Flights>,
    output: Arc<Output>,
    lane: SyncSender<Job>,
    control: Arc<Control>,
}

/// `tools/call`'s params, each member raw, present or not.
#[derive(Deserialize)]
struct CallParams<'a> {
    #[serde(default, borrow, deserialize_with = "frame::present")]
    name: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "frame::present")]
    arguments: Option<&'a RawValue>,
    #[serde(default, borrow, rename = "_meta", deserialize_with = "frame::present")]
    meta: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Meta<'a> {
    #[serde(
        default,
        borrow,
        rename = "progressToken",
        deserialize_with = "frame::present"
    )]
    progress_token: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct CancelledParams<'a> {
    #[serde(
        default,
        borrow,
        rename = "requestId",
        deserialize_with = "frame::present"
    )]
    request_id: Option<&'a RawValue>,
}

/// An `invalid_input` error saying `message`.
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidInput, message)
}

/// `text` quoted, cut to its first [`QUOTED_AT_MOST`] characters.
fn quoted(text: &str) -> String {
    match text.char_indices().nth(QUOTED_AT_MOST) {
        Some((at, _)) => format!("{}...", silico_core::command::quoted(&text[..at])),
        None => silico_core::command::quoted(text),
    }
}

/// Whether `command` waits on a session, and so takes a slot: every command matched, so a
/// new one must say.
fn takes_a_slot(command: Command) -> bool {
    match command {
        // Answered from what the sessions hold when asked, never queued.
        Command::Version | Command::Status | Command::Cancel | Command::Schema => false,
        Command::Open
        | Command::Close
        | Command::Begin
        | Command::Commit
        | Command::Rollback
        | Command::Format
        | Command::AddFeature
        | Command::SetField
        | Command::RemoveFeature
        | Command::ReplaceText
        | Command::Text
        | Command::Outline
        | Command::Check
        | Command::Elements
        | Command::Query
        | Command::MassProperties
        | Command::Distance
        | Command::Angle
        | Command::Render => true,
    }
}

impl<P: Clients + 'static> Dispatch<'_, P> {
    /// Queues `text` for the output, waiting while the queue is full: unless the server is
    /// ending, whose end mustn't wait on a client that stopped reading.
    fn queue(&self, mut job: Job) {
        loop {
            match self.lane.try_send(job) {
                Ok(()) => return,
                Err(TrySendError::Full(again)) => {
                    if self.control.ending() {
                        return;
                    }
                    job = again;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    fn reply(&self, id: &Id, result: &str) {
        let mut text = Vec::new();
        let _ = reply::result(id, result)(&mut text);
        self.queue(Job::Text(String::from_utf8_lossy(&text).into_owned()));
    }

    fn error(&self, id: Option<&Id>, code: i32, message: &str, error: Error) {
        self.queue(Job::Text(reply::error(
            id,
            code,
            message,
            &Problems::from(error),
        )));
    }

    /// A tool's answer, written now (in turn with the other replies): a refusal before the
    /// command ran.
    fn refuse_call(&self, id: &Id, error: Error) {
        let answered = Answered {
            ok: false,
            text: reply::refused(&Problems::from(error)),
            png: None,
        };
        let mut text = Vec::new();
        let _ = reply::answer(id, &answered)(&mut text);
        self.queue(Job::Text(String::from_utf8_lossy(&text).into_owned()));
    }

    fn message(&mut self, line: &[u8]) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        match frame::parse(line) {
            Err(Bad::Parse(why)) => self.error(
                None,
                PARSE_ERROR,
                "Parse error",
                invalid(why).with_suggestion(
                    "send one JSON-RPC message a line, each strict JSON (RFC 8259) in UTF-8",
                ),
            ),
            Err(Bad::Invalid { id, why }) => self.error(
                id.as_ref(),
                INVALID_REQUEST,
                "Invalid Request",
                invalid(why).with_suggestion(
                    "send a JSON-RPC 2.0 request: an object with jsonrpc \"2.0\", an id that's a string or a number, a method, and its params",
                ),
            ),
            Ok(Message::Response) => tracing::debug!("a response, to nothing asked: ignored"),
            Ok(Message::Notification { method, params }) => match method.as_str() {
                "notifications/cancelled" => self.cancelled(params),
                "notifications/initialized" => {}
                _ => tracing::debug!(method, "a notification not handled: ignored"),
            },
            Ok(Message::Request { id, method, params }) => self.request(&id, &method, params),
        }
    }

    fn too_long(&mut self, head: &[u8], length: u64) {
        let id = frame::find_id(head);
        tracing::warn!(length, "a message too long to read: skipped");
        self.error(
            id.as_ref(),
            INVALID_REQUEST,
            "Invalid Request",
            invalid(format!(
                "the message is {length} bytes, more than the {} a message may be",
                frame::MAX_LINE
            ))
            .with_suggestion(format!(
                "send less at once: a call's arguments are at most {} bytes",
                silico_core::document::json::MAX_INPUT_LEN
            )),
        );
    }

    fn cancelled(&self, params: Option<&RawValue>) {
        let id = params
            .filter(|p| p.get().starts_with('{'))
            .and_then(|p| serde_json::from_str::<CancelledParams<'_>>(p.get()).ok())
            .and_then(|p| p.request_id)
            .and_then(Id::of);
        match id {
            Some(id) => {
                let cancelled = self.flights.cancel(&id);
                tracing::debug!(id = id.raw(), cancelled, "cancellation");
            }
            None => tracing::debug!("a cancellation naming no request: ignored"),
        }
    }

    fn request(&mut self, id: &Id, method: &str, params: Option<&RawValue>) {
        if self.flights.contains(id) {
            self.error(
                Some(id),
                INVALID_REQUEST,
                "Invalid Request",
                invalid(format!(
                    "the ID {} is a request's still in flight",
                    quoted(id.raw())
                ))
                .with_suggestion("give each request an ID no request in flight has"),
            );
            return;
        }
        match method {
            "initialize" => {
                self.initialized = true;
                self.reply(id, &self.initialized_text);
            }
            "ping" => self.reply(id, "{}"),
            "tools/list" => self.reply(id, &self.tools),
            "tools/call" => self.call(id, params),
            _ => self.error(
                Some(id),
                METHOD_NOT_FOUND,
                "Method not found",
                invalid(format!("no method is named {}", quoted(method)))
                    .with_parameter("method")
                    .with_candidates(METHODS)
                    .with_suggestion("call a tool with tools/call: tools/list lists them"),
            ),
        }
    }

    fn invalid_params(&self, id: &Id, error: Error) {
        self.error(Some(id), INVALID_PARAMS, "Invalid params", error);
    }

    fn call(&mut self, id: &Id, params: Option<&RawValue>) {
        if !self.initialized {
            self.error(
                Some(id),
                INVALID_REQUEST,
                "Invalid Request",
                invalid("tools/call came before initialize")
                    .with_parameter("method")
                    .with_suggestion(
                        "send initialize first, then notifications/initialized, then call tools",
                    ),
            );
            return;
        }
        // An object, as MCP gives them (serde would take an array as the struct's fields
        // by place).
        let params = match params.map(|p| {
            if p.get().starts_with('{') {
                serde_json::from_str::<CallParams<'_>>(p.get()).map_err(|e| e.to_string())
            } else {
                Err("got another value".to_owned())
            }
        }) {
            None => CallParams {
                name: None,
                arguments: None,
                meta: None,
            },
            Some(Ok(params)) => params,
            Some(Err(e)) => {
                return self.invalid_params(
                    id,
                    invalid(format!(
                        "tools/call's params must be an object, each key once: {e}"
                    ))
                    .with_parameter("params")
                    .with_suggestion(
                        "give tools/call's params as an object: the tool's name, its arguments, and _meta if you want progress",
                    ),
                );
            }
        };
        let names = || tools_named();
        let name = match params.name.map(|n| serde_json::from_str::<String>(n.get())) {
            None => {
                return self.invalid_params(
                    id,
                    invalid("name is missing")
                        .with_parameter("name")
                        .with_candidates(names())
                        .with_suggestion("name the tool to call"),
                );
            }
            Some(Err(_)) => {
                return self.invalid_params(
                    id,
                    invalid("name must be a string")
                        .with_parameter("name")
                        .with_candidates(names())
                        .with_suggestion("name the tool to call"),
                );
            }
            Some(Ok(name)) => name,
        };
        let Some(command) = Command::named(&name) else {
            return self.invalid_params(
                id,
                invalid(format!("no tool is named {}", quoted(&name)))
                    .with_parameter("name")
                    .with_candidates(names())
                    .with_suggestion("call one of the tools tools/list gives"),
            );
        };
        let arguments = params.arguments.map_or("{}", RawValue::get);
        if !arguments.starts_with('{') {
            return self.invalid_params(
                id,
                invalid(format!(
                    "arguments must be an object, got {}",
                    quoted(arguments)
                ))
                .with_parameter("arguments")
                .with_suggestion("give the tool's request as an object, as its inputSchema says"),
            );
        }
        let token = match params.meta {
            None => None,
            Some(meta) => match Some(meta.get())
                .filter(|m| m.starts_with('{'))
                .map(serde_json::from_str::<Meta<'_>>)
            {
                None | Some(Err(_)) => {
                    return self.invalid_params(
                        id,
                        invalid("_meta must be an object, each key once")
                            .with_parameter("_meta")
                            .with_suggestion(
                                "give _meta as an object, each key once (its progressToken a string or a number), or leave it out",
                            ),
                    );
                }
                Some(Ok(Meta {
                    progress_token: None,
                })) => None,
                Some(Ok(Meta {
                    progress_token: Some(token),
                })) => match Id::of(token) {
                    Some(_) => Some(token.get().to_owned()),
                    None => {
                        return self.invalid_params(
                            id,
                            invalid("_meta.progressToken must be a string or a number")
                                .with_parameter("_meta.progressToken")
                                .with_suggestion(
                                    "give a progress token that's a string or a number, or leave it out for no progress",
                                ),
                        );
                    }
                },
            },
        };
        let slotted = takes_a_slot(command);
        let Some(landing) = self.flights.take(id, slotted) else {
            return self.refuse_call(
                id,
                invalid(format!(
                    "{MAX_IN_FLIGHT} calls are already running in this server, the most it runs at once"
                ))
                .with_suggestion("wait for one to answer, or cancel one, and call again"),
            );
        };
        let waiter = Waiter {
            id: id.clone(),
            landing,
            token,
            output: Arc::clone(&self.output),
        };
        if slotted {
            // The waiter's thread first, so a call is only started once it can be waited for.
            let (give, calling) = mpsc::sync_channel::<Calling>(1);
            let flight = Arc::clone(&waiter.landing.flight);
            let started = std::thread::Builder::new()
                .name(format!("mcp-call-{}", command.as_str()))
                .spawn(move || {
                    if let Ok(calling) = calling.recv() {
                        waiter.run(calling);
                    }
                });
            if let Err(e) = started {
                // The waiter, and its landing, dropped with the closure.
                return self.refuse_call(
                    id,
                    Error::new(
                        ErrorCode::ResourceExhausted,
                        format!("couldn't start a thread to wait for the call: {e}"),
                    )
                    .with_suggestion("wait for a call to answer, and call again"),
                );
            }
            match self.sessions.call(&name, arguments) {
                Call::Running(running) => {
                    *lock(&flight.stopper) = Some(running.stopper());
                    let _ = give.send(running);
                }
                Call::Unknown => {
                    tracing::error!(name, "a command named, then not one: Silico's bug");
                    drop(give);
                }
            }
        } else {
            match self.sessions.call(&name, arguments) {
                Call::Running(running) => {
                    *lock(&waiter.landing.flight.stopper) = Some(running.stopper());
                    self.queue(Job::Call(waiter, running));
                }
                Call::Unknown => {
                    tracing::error!(name, "a command named, then not one: Silico's bug");
                    drop(waiter);
                }
            }
        }
    }
}

/// Every tool's name, in the list's order.
fn tools_named() -> Vec<&'static str> {
    Command::ALL.iter().map(|c| c.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_counts_each_step_seen_done_and_never_falls() {
        // A check behind an edit: the edit's 9 of 9, then its own from 0 to 12.
        let mut count = 0;
        let mut last = 0;
        let mut counts = Vec::new();
        for done in [3, 9, 9, 0, 1, 1, 5, 12] {
            count = grown(count, last, done);
            last = done;
            counts.push(count);
        }
        assert_eq!(counts, [3, 9, 9, 9, 10, 10, 14, 21]);
    }
}
