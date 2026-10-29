//! One entry for every command by name (docs/design/m4.9-mcp.md, *What's added* and
//! *`Call`*): [`Sessions::call`], given a command's name and its request's text, as a
//! transport has them (MCP's `tools/call`), answers with the command's envelope, as
//! `wire::response_text` writes it on one line, and `render`'s PNG beside it. It's built
//! only on the typed methods (`Sessions::open`, `check`, `distance`, ...), so it does
//! nothing a client of those can't: the request is read by Silico's own reader, as a
//! document is (a key given twice, a number with more digits than its quantity holds, a
//! nesting past 64, or more than 64 MiB, all refused, never rounded or half-read), then by
//! its command's `*Request::read`, and the method called. A request either reader refuses is
//! answered with the core's refused envelope, as any refusal is: carrying the state of the
//! session its `session` field names, if that reads as an open session's ID (the field read
//! alone, before the rest, and only to find the session: nothing runs on it), as any other
//! refusal of the command would, and `null` otherwise.
//!
//! The names are one enum, [`Command`], matched whole, from which `wire::COMMANDS` (the
//! snapshots, MCP's tools) is made, so no command is listed and not run, or run and not
//! listed.
//!
//! `call` never waits on a session: a command it starts is queued on its session (or, for a
//! read of the published model, started on a thread of its own) and runs off the caller's
//! thread, so commands a caller sends to one session reach it in the order sent, and `open`s
//! take their session IDs in that order. `status`, `cancel`, `read_logs`, `version`, and
//! `schema`, which take no place in a session's queue, are answered when the [`Calling`] is
//! waited on, on the waiting thread, so the caller (MCP's reader) never takes what they
//! lock.

use std::time::Duration;

use super::render::Rendering;
use super::session::WeakSessions;
use super::wire::edits::{
    AddFeatureRequest, MoveFeatureRequest, RemoveFeatureRequest, ReplaceTextRequest,
    SetFieldRequest,
};
use super::wire::elements::{ElementsRequest, QueryRequest};
use super::wire::export::ExportRequest;
use super::wire::logs::ReadLogsRequest;
use super::wire::measure::{MassPropertiesRequest, PairRequest};
use super::wire::reads::{CheckRequest, TextRequest, TimelineRequest};
use super::wire::render::RenderRequest;
use super::wire::schema::SchemaRequest;
use super::wire::session::{CancelRequest, OpenRequest, VersionRequest};
use super::wire::sketch::SketchRequest;
use super::wire::sketch_edits::{
    SketchAddRequest, SketchMergeRequest, SketchRemoveRequest, SketchSplitRequest,
    SketchTrimRequest, SketchUnjoinRequest,
};
use super::wire::transaction::{FormatRequest, SessionRequest};
use super::wire::{self, Json, Layout, ToWire};
use super::{
    Problems, Progress, Response, Running, SessionId, SessionInfo, Sessions, Stopper, VERSION,
};
use crate::document::json::{self, Cause, MAX_INPUT_LEN, Node, SyntaxError, Value};
use crate::kernel::Clients;
use crate::{Error, ErrorCode};

/// Every command, by the name requests and tools give it, in the design's order (section 7):
/// what [`Sessions::call`] runs, and what `wire::COMMANDS` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    Version,
    Open,
    Close,
    Status,
    Cancel,
    Begin,
    Commit,
    Rollback,
    Format,
    AddFeature,
    SetField,
    RemoveFeature,
    ReplaceText,
    Text,
    Outline,
    Check,
    CacheClear,
    Elements,
    Query,
    Schema,
    MassProperties,
    Distance,
    Angle,
    Render,
    ReadLogs,
    Export,
    MoveFeature,
    Timeline,
    Sketch,
    SketchAdd,
    SketchRemove,
    SketchSplit,
    SketchTrim,
    SketchUnjoin,
    SketchMerge,
}

impl Command {
    /// Every command, in the design's order: M4.1's (section 7), then M5.0's (its section
    /// 7).
    pub const ALL: [Command; 35] = [
        Command::Version,
        Command::Open,
        Command::Close,
        Command::Status,
        Command::Cancel,
        Command::Begin,
        Command::Commit,
        Command::Rollback,
        Command::Format,
        Command::AddFeature,
        Command::SetField,
        Command::RemoveFeature,
        Command::ReplaceText,
        Command::Text,
        Command::Outline,
        Command::Check,
        Command::CacheClear,
        Command::Elements,
        Command::Query,
        Command::Schema,
        Command::MassProperties,
        Command::Distance,
        Command::Angle,
        Command::Render,
        Command::ReadLogs,
        Command::Export,
        Command::MoveFeature,
        Command::Timeline,
        Command::Sketch,
        Command::SketchAdd,
        Command::SketchRemove,
        Command::SketchSplit,
        Command::SketchTrim,
        Command::SketchUnjoin,
        Command::SketchMerge,
    ];

    /// The command's name, as requests and tools give it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Command::Version => "version",
            Command::Open => "open",
            Command::Close => "close",
            Command::Status => "status",
            Command::Cancel => "cancel",
            Command::Begin => "begin",
            Command::Commit => "commit",
            Command::Rollback => "rollback",
            Command::Format => "format",
            Command::AddFeature => "add_feature",
            Command::SetField => "set_field",
            Command::RemoveFeature => "remove_feature",
            Command::ReplaceText => "replace_text",
            Command::Text => "text",
            Command::Outline => "outline",
            Command::Check => "check",
            Command::CacheClear => "cache_clear",
            Command::Elements => "elements",
            Command::Query => "query",
            Command::Schema => "schema",
            Command::MassProperties => "mass_properties",
            Command::Distance => "distance",
            Command::Angle => "angle",
            Command::Render => "render",
            Command::ReadLogs => "read_logs",
            Command::Export => "export",
            Command::MoveFeature => "move_feature",
            Command::Timeline => "timeline",
            Command::Sketch => "sketch",
            Command::SketchAdd => "sketch_add",
            Command::SketchRemove => "sketch_remove",
            Command::SketchSplit => "sketch_split",
            Command::SketchTrim => "sketch_trim",
            Command::SketchUnjoin => "sketch_unjoin",
            Command::SketchMerge => "sketch_merge",
        }
    }

    /// The command named `name`, exactly (no other spelling): none for a name no command has.
    pub fn named(name: &str) -> Option<Command> {
        Command::ALL
            .into_iter()
            .find(|command| command.as_str() == name)
    }

    /// Whether the command is answered when its [`Calling`] is waited on, on the waiting
    /// thread, from what the sessions hold then: taking no place in a session's queue, it
    /// never waits on one (so a transport needn't give it a thread or a slot of its own).
    /// `status`, `cancel`, `read_logs`, `version`, and `schema`; every other command runs off the
    /// caller's thread, and is waited on. Every command matched, so a new one must say.
    pub const fn answered_when_waited(self) -> bool {
        match self {
            Command::Version
            | Command::Status
            | Command::Cancel
            | Command::ReadLogs
            | Command::Schema => true,
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
            | Command::CacheClear
            | Command::Elements
            | Command::Query
            | Command::MassProperties
            | Command::Distance
            | Command::Angle
            | Command::Render
            | Command::Export
            | Command::MoveFeature
            | Command::Timeline
            | Command::Sketch
            | Command::SketchAdd
            | Command::SketchRemove
            | Command::SketchSplit
            | Command::SketchTrim
            | Command::SketchUnjoin
            | Command::SketchMerge => false,
        }
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What [`Sessions::call`] answers.
#[must_use = "a call's answer is only had by waiting on it"]
pub enum Call {
    /// No command has this name: a transport's own error (MCP's JSON-RPC error), not an
    /// envelope (M4.1 section 9).
    Unknown,
    /// The command, running: its answer to be waited on.
    Running(Calling),
}

/// A command started by name, of any type: what a transport needs of it. Its answer is
/// the command's envelope, as the typed method's response is written on one line.
#[must_use = "a call's answer is only had by waiting on it"]
pub struct Calling {
    pending: Pending,
}

/// How a [`Calling`] comes to its answer.
enum Pending {
    /// A command running off the caller's thread (queued on its session, or a read of the
    /// published model on its own thread).
    Waiting(Box<dyn Waiting>),
    /// A command answered when it's waited on, on the waiting thread: `status`, `cancel`,
    /// `read_logs`, `version`, and `schema`, which take no place in a session's queue, and a
    /// request refused before it ran, which finds the session it names then.
    Later(Box<dyn FnOnce() -> Answered + Send>),
}

/// A command's [`Running`] handle, of whatever result type, with how its response is
/// written.
trait Waiting: Send {
    fn progress(&self) -> Option<Progress>;
    fn stopper(&self) -> Stopper;
    fn wait_timeout(self: Box<Self>, timeout: Duration) -> Result<Answered, Box<dyn Waiting>>;
    fn wait(self: Box<Self>) -> Answered;
}

/// A [`Running`] handle, and what makes its answer from its response.
struct Typed<T> {
    running: Running<T>,
    answer: fn(Response<T>) -> Answered,
}

impl<T: Send + 'static> Waiting for Typed<T> {
    fn progress(&self) -> Option<Progress> {
        self.running.progress()
    }

    fn stopper(&self) -> Stopper {
        self.running.stopper()
    }

    fn wait_timeout(self: Box<Self>, timeout: Duration) -> Result<Answered, Box<dyn Waiting>> {
        let Typed { running, answer } = *self;
        match running.wait_timeout(timeout) {
            Ok(response) => Ok(answer(response)),
            Err(running) => Err(Box::new(Typed { running, answer })),
        }
    }

    fn wait(self: Box<Self>) -> Answered {
        (self.answer)(self.running.wait())
    }
}

impl Calling {
    /// Whether it waits on nothing: answered when it's waited on, on the waiting thread, a
    /// command whose [`Command::answered_when_waited`] says so, or a request refused before
    /// it ran. Not a transport's routing rule, which must be decided before the call starts
    /// ([`Command::answered_when_waited`]): a refused request is known only after.
    pub fn waits_on_nothing(&self) -> bool {
        matches!(self.pending, Pending::Later(_))
    }

    /// How far its session's rebuild has got, as the typed handle's
    /// [`Running::progress`] says: none for a command answered when it's waited on, or
    /// refused before it ran.
    pub fn progress(&self) -> Option<Progress> {
        match &self.pending {
            Pending::Waiting(waiting) => waiting.progress(),
            Pending::Later(_) => None,
        }
    }

    /// What cancels it from another thread, as the typed handle's [`Running::stopper`]
    /// does: one that stops nothing for a command answered when it's waited on (which never
    /// waits on work to stop), or refused before it ran.
    pub fn stopper(&self) -> Stopper {
        match &self.pending {
            Pending::Waiting(waiting) => waiting.stopper(),
            Pending::Later(_) => Stopper::none(),
        }
    }

    /// Waits up to `timeout` for the answer, or gives the call back to wait on again. A
    /// command answered when it's waited on answers now, whatever `timeout` is.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Answered, Calling> {
        match self.pending {
            Pending::Waiting(waiting) => waiting.wait_timeout(timeout).map_err(|waiting| Calling {
                pending: Pending::Waiting(waiting),
            }),
            Pending::Later(answer) => Ok(answer()),
        }
    }

    /// Waits for the answer.
    pub fn wait(self) -> Answered {
        match self.pending {
            Pending::Waiting(waiting) => waiting.wait(),
            Pending::Later(answer) => answer(),
        }
    }

    /// `running`'s answer, each response written by `answer`.
    fn waiting<T: Send + 'static>(
        running: Running<T>,
        answer: fn(Response<T>) -> Answered,
    ) -> Self {
        Calling {
            pending: Pending::Waiting(Box::new(Typed { running, answer })),
        }
    }

    /// What `answer` gives, when the call is waited on.
    fn later(answer: impl FnOnce() -> Answered + Send + 'static) -> Self {
        Calling {
            pending: Pending::Later(Box::new(answer)),
        }
    }

    /// A request refused before it ran, for every reason in `problems`, carrying the state
    /// of the session it names, found when it's waited on.
    fn refused(named: Named, problems: Problems) -> Self {
        Calling::later(move || Answered::refused_in(named.and_then(|info| info()), problems))
    }
}

/// The session a request names in its `session` field, read alone: what finds its state
/// when the request's refusal is waited on, on the waiting thread, as `status` reads it
/// (never on the caller's, which never waits on a session), none if it isn't open then.
/// None for a command that takes no session, or a field that isn't a session's ID.
type Named = Option<Box<dyn FnOnce() -> Option<SessionInfo> + Send>>;

/// A command's answer, as a transport sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// Whether the command ran (the envelope's `ok`): false for a refusal, and for an answer
    /// refused as too long to write.
    pub ok: bool,
    /// The envelope, on one line (`wire::response_text` with `Layout::OneLine`).
    pub text: String,
    /// `render`'s PNG, if it rendered: beside the envelope, never in it.
    pub png: Option<Vec<u8>>,
}

impl Answered {
    /// The answer of a request refused before it ran, for every reason in `problems`: the
    /// core's refused envelope, with no session and no model, on one line, as every
    /// refusal is written (a transport's own refusal of a call, or its error's `data`).
    pub fn refused(problems: Problems) -> Answered {
        Answered::refused_in(None, problems)
    }

    /// The answer of a request refused before it ran, for every reason in `problems`,
    /// carrying `session`, the state of the open session it names.
    fn refused_in(session: Option<SessionInfo>, problems: Problems) -> Answered {
        answered(Response::<Unread> {
            session,
            model: None,
            result: Err(problems),
        })
    }
}

/// `response` as [`Answered`]: its envelope on one line, and whether it ran.
fn answered<T: ToWire>(response: Response<T>) -> Answered {
    let (ok, text) = wire::answer_text(&response, Layout::OneLine);
    Answered {
        ok,
        text,
        png: None,
    }
}

/// `render`'s response as [`Answered`]: the envelope its result makes, and its PNG beside
/// it, if it rendered.
fn rendered(response: Response<Rendering>) -> Answered {
    let Response {
        session,
        model,
        result,
    } = response;
    let (result, png) = match result {
        Ok(Rendering { result, png }) => (Ok(result), Some(png)),
        Err(problems) => (Err(problems), None),
    };
    let answer = answered(Response {
        session,
        model,
        result,
    });
    Answered {
        // None for an answer refused as too long to write, whose envelope holds no result.
        png: png.filter(|_| answer.ok),
        ..answer
    }
}

/// The result of a request no reader took: it has none, so its response is only ever a
/// refusal.
enum Unread {}

impl ToWire for Unread {
    fn to_wire(&self) -> Json<'_> {
        match *self {}
    }
}

/// The refusal of a request's text that Silico's reader refused, as the document's reader
/// says it: too large to read (`invalid_input`, no parameter, as section 6's limits are), or
/// not strict JSON, and where.
fn unread(e: SyntaxError) -> Error {
    match e.cause {
        Cause::Size => Error::new(
            ErrorCode::InvalidInput,
            format!("the request is too large for Silico to read: {}", e.message),
        )
        .with_suggestion(format!(
            "send less at once: a request is at most {MAX_INPUT_LEN} bytes"
        )),
        Cause::Text => Error::new(
            ErrorCode::InvalidInput,
            format!("the request isn't valid JSON: {}, at {}", e.message, e.at),
        )
        .with_suggestion(
            "fix the request there: a request is strict JSON (RFC 8259), each key given once",
        ),
    }
}

/// The command `request` asks for, started by `run`, its answer written as its result is;
/// or, if the command's reader refused the request, its refusal.
fn typed<R, T: ToWire + Send + 'static>(
    named: Named,
    request: Result<R, Problems>,
    run: impl FnOnce(R) -> Running<T>,
) -> Calling {
    match request {
        Ok(request) => Calling::waiting(run(request), answered),
        Err(problems) => Calling::refused(named, problems),
    }
}

/// The command `request` asks for, answered by `answer` when it's waited on; or, if the
/// command's reader refused the request, its refusal.
fn later<R: Send + 'static, T: ToWire>(
    named: Named,
    request: Result<R, Problems>,
    answer: impl FnOnce(R) -> Response<T> + Send + 'static,
) -> Calling {
    match request {
        Ok(request) => Calling::later(move || answered(answer(request))),
        Err(problems) => Calling::refused(named, problems),
    }
}

/// The session `request`'s `session` field names, read alone: its ID, if the request is an
/// object holding the field and it's a string that reads as one; none otherwise.
fn session_named(request: &Node) -> Option<SessionId> {
    let Value::Object(members) = &request.value else {
        return None;
    };
    members
        .iter()
        .find(|member| member.key == "session")
        .and_then(|member| match &member.value.value {
            Value::String(text) => SessionId::parse(text),
            _ => None,
        })
}

impl<P: Clients + 'static> Sessions<P> {
    /// Runs the command named `name` (exactly as [`Command::as_str`] gives it) on
    /// `arguments`, as [`call_command`](Self::call_command) does: [`Call::Unknown`] for a
    /// name no command has.
    pub fn call(&self, name: &str, arguments: &str) -> Call {
        match Command::named(name) {
            Some(command) => Call::Running(self.call_command(command, arguments)),
            None => Call::Unknown,
        }
    }

    /// Runs `command` on `arguments`, its request's text, read by Silico's own reader, then
    /// by the command's request reader, and its typed method called: the command running,
    /// whose answer is its envelope (refused, `ok` false, for a request either reader
    /// refuses). Never waits on a session: the command is queued (or, a published read,
    /// started) before this returns, so a caller's commands to one session run in the
    /// order it sent them. `status`, `cancel`, `read_logs`, `version`, and `schema` are
    /// answered when the call is waited on, on the waiting thread
    /// ([`Command::answered_when_waited`]).
    pub fn call_command(&self, command: Command, arguments: &str) -> Calling {
        let _span = tracing::info_span!("call", command = command.as_str()).entered();
        match json::read(arguments.as_bytes()) {
            Ok(request) => self.dispatch(command, &request),
            Err(e) => Calling::refused(None, Problems::from(unread(e))),
        }
    }

    /// What finds the state of the session `request` names, for its refusal: none for a
    /// command that takes no session (`version`, `open`, `schema`), whose refusals never
    /// carry one.
    fn named(&self, command: Command, request: &Node) -> Named {
        if matches!(command, Command::Version | Command::Open | Command::Schema) {
            return None;
        }
        let session = session_named(request)?;
        let sessions: WeakSessions<P> = self.weak();
        Some(Box::new(move || sessions.info(&session)))
    }

    /// `command` run on `request`, through its typed method.
    fn dispatch(&self, command: Command, request: &Node) -> Calling {
        let named = self.named(command, request);
        match command {
            Command::Version => later(named, VersionRequest::read(request), |VersionRequest| {
                Response {
                    session: None,
                    model: None,
                    result: Ok(VERSION),
                }
            }),
            Command::Open => typed(named, OpenRequest::read(request), |r| {
                self.open(r.path, r.create)
            }),
            Command::Close => typed(named, SessionRequest::read(request), |r| {
                self.close(&r.session)
            }),
            Command::Status => {
                let sessions: WeakSessions<P> = self.weak();
                later(named, SessionRequest::read(request), move |r| {
                    sessions.status(&r.session)
                })
            }
            Command::Cancel => {
                let sessions: WeakSessions<P> = self.weak();
                later(named, CancelRequest::read(request), move |r| {
                    sessions.cancel(&r.session, r.command)
                })
            }
            Command::Begin => typed(named, SessionRequest::read(request), |r| {
                self.begin(&r.session)
            }),
            Command::Commit => typed(named, SessionRequest::read(request), |r| {
                self.commit(&r.session)
            }),
            Command::Rollback => typed(named, SessionRequest::read(request), |r| {
                self.rollback(&r.session)
            }),
            Command::Format => typed(named, FormatRequest::read(request), |r| {
                self.format(&r.session, r.features, r.write)
            }),
            Command::AddFeature => typed(named, AddFeatureRequest::read(request), |r| {
                self.add_feature(&r.session, r.revision, r.before, r.feature)
            }),
            Command::SetField => typed(named, SetFieldRequest::read(request), |r| {
                self.set_field(&r.session, r.revision, r.feature, r.field, r.value)
            }),
            Command::RemoveFeature => typed(named, RemoveFeatureRequest::read(request), |r| {
                self.remove_feature(&r.session, r.revision, r.feature)
            }),
            Command::ReplaceText => typed(named, ReplaceTextRequest::read(request), |r| {
                self.replace_text(&r.session, r.revision, r.text)
            }),
            Command::Text => typed(named, TextRequest::read(request), |r| {
                self.text(&r.session, r.feature)
            }),
            Command::Outline => typed(named, SessionRequest::read(request), |r| {
                self.outline(&r.session)
            }),
            Command::Check => typed(named, CheckRequest::read(request), |r| {
                self.check(&r.session, r.model)
            }),
            Command::CacheClear => typed(named, SessionRequest::read(request), |r| {
                self.cache_clear(&r.session)
            }),
            Command::Elements => typed(named, ElementsRequest::read(request), |r| {
                self.elements(&r.session, r.model, r.feature, r.after, r.limit)
            }),
            Command::Query => typed(named, QueryRequest::read(request), |r| {
                self.query(&r.session, r.model, r.query)
            }),
            Command::Schema => later(named, SchemaRequest::read(request), |r| Response {
                session: None,
                model: None,
                result: super::schema::schema(&r.of),
            }),
            Command::MassProperties => typed(named, MassPropertiesRequest::read(request), |r| {
                self.mass_properties(&r.session, r.model, r.feature)
            }),
            Command::Distance => typed(named, PairRequest::read(request), |r| {
                self.distance(&r.session, r.model, r.elements, r.feature)
            }),
            Command::Angle => typed(named, PairRequest::read(request), |r| {
                self.angle(&r.session, r.model, r.elements, r.feature)
            }),
            Command::Render => match RenderRequest::read(request) {
                Ok(r) => Calling::waiting(self.render(&r.session, r.model, r.render), rendered),
                Err(problems) => Calling::refused(named, problems),
            },
            Command::ReadLogs => {
                let sessions: WeakSessions<P> = self.weak();
                later(named, ReadLogsRequest::read(request), move |r| {
                    sessions.read_logs(&r.session, &r.query)
                })
            }
            Command::Export => typed(named, ExportRequest::read(request), |r| {
                self.export(&r.session, r.export)
            }),
            Command::MoveFeature => typed(named, MoveFeatureRequest::read(request), |r| {
                self.move_feature(&r.session, r.revision, r.feature, r.before)
            }),
            Command::Timeline => typed(named, TimelineRequest::read(request), |r| {
                self.timeline(&r.session, r.model)
            }),
            Command::Sketch => typed(named, SketchRequest::read(request), |r| {
                self.sketch(&r.session, r.model, r.sketch)
            }),
            Command::SketchAdd => typed(named, SketchAddRequest::read(request), |r| {
                self.sketch_add(&r.session, r.revision, r.sketch, r.members)
            }),
            Command::SketchRemove => typed(named, SketchRemoveRequest::read(request), |r| {
                self.sketch_remove(&r.session, r.revision, r.sketch, r.members)
            }),
            Command::SketchSplit => typed(SketchSplitRequest::read(request), |r| {
                self.sketch_split(&r.session, r.revision, r.sketch, r.curve, r.at, r.ids)
            }),
            Command::SketchTrim => typed(SketchTrimRequest::read(request), |r| {
                self.sketch_trim(&r.session, r.revision, r.sketch, r.curve, r.pick, r.ids)
            }),
            Command::SketchUnjoin => typed(SketchUnjoinRequest::read(request), |r| {
                self.sketch_unjoin(&r.session, r.revision, r.sketch, r.point, r.curve, r.ids)
            }),
            Command::SketchMerge => typed(SketchMergeRequest::read(request), |r| {
                self.sketch_merge(&r.session, r.revision, r.sketch, r.points)
            }),
        }
    }
}
