//! `version`, `open`, `close`, `status`, and `cancel`'s wire types
//! (docs/design/m4.1-command-api.md, sections 3, 6, and 7): their requests, read strictly
//! ([`VersionRequest`], [`OpenRequest`], [`CancelRequest`], and the
//! [`SessionRequest`](super::transaction::SessionRequest) `close` and `status` take); their
//! results, written field for field ([`ToWire`]); and the mirrors their snapshots,
//! `commands/version.json`, `open.json`, `close.json`, `status.json`, and `cancel.json`, are
//! generated from. `version` takes no session, so its envelope's `session` is `null`. What
//! the values mean (a file that isn't there, a command that isn't running) the session says.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

#[cfg(doc)]
use super::COMMANDS;
use super::transaction::TransactionSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape, words};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    Cancellable, Cancelled, Closed, Limits, Opened, Problems, ReadCommand, Rebuild, RunningCommand,
    SessionCommand, SessionId, Status, Version,
};
use crate::document::json::Node;
use crate::document::{FeatureId, Length, PROBLEMS_LISTED};
use crate::{Error, ErrorCode};

/// A `version` request: it has no fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRequest;

impl VersionRequest {
    /// The request `node` holds, an object with no members, or every reason it's refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |_| Some(VersionRequest))
    }
}

impl ToWire for VersionRequest {
    /// `{}`.
    fn to_wire(&self) -> Json<'_> {
        Json::object([])
    }
}

/// An `open` request: the document's path, UTF-8 (a response names it exactly), or none for
/// an untitled document, and whether to make a new, empty document there (with no path,
/// always: an untitled document is a new one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: Option<String>,
    pub create: bool,
}

impl OpenRequest {
    /// The request `node` holds, or every reason its shape is refused: as well as each
    /// field's own, `path` null with `create` false, which opens nothing, as fields that
    /// must agree are refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let path = f.nullable_string("path");
            let create = f.bool("create");
            let (path, create) = (path?, create?);
            if path.is_none() && !create {
                f.refuse(
                    Error::new(ErrorCode::InvalidInput, "path null opens nothing")
                        .with_parameter("path")
                        .with_suggestion(
                            "give a path to open, or create: true for an untitled document",
                        ),
                );
                return None;
            }
            Some(OpenRequest {
                path: path.map(str::to_owned),
                create,
            })
        })
    }
}

impl ToWire for OpenRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("path", Json::or_null(self.path.as_deref(), Json::str)),
            ("create", Json::bool(self.create)),
        ])
    }
}

/// The commands that run in a session, each by its name, in the design's order (a subset of
/// [`COMMANDS`]): what `status` says is running, and `cancel` stops
/// ([`SessionCommand::ALL`]).
pub const SESSION_COMMANDS: [&str; SessionCommand::ALL.len()] = {
    let mut names = [""; SessionCommand::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = SessionCommand::ALL[at].as_str();
        at += 1;
    }
    names
};

/// The commands that may run as reads of the published model ([`ReadCommand::ALL`]), each
/// by name, in the design's order (a subset of [`SESSION_COMMANDS`]): what `status` lists
/// among the published reads running.
pub const PUBLISHED_READS: [&str; ReadCommand::ALL.len()] = {
    let mut names = [""; ReadCommand::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = ReadCommand::ALL[at].as_str();
        at += 1;
    }
    names
};

/// What `cancel` names, each by its name, in the design's order: every command that runs in
/// a session ([`SESSION_COMMANDS`]), then `rebuild` ([`Cancellable::ALL`]).
pub const CANCELLABLE: [&str; Cancellable::ALL.len()] = {
    let mut names = [""; Cancellable::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = Cancellable::ALL[at].as_str();
        at += 1;
    }
    names
};

/// A `cancel` request: the session, and what to stop, by its name: one of the commands that
/// run in a session, or `rebuild` ([`CANCELLABLE`]), never any other string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelRequest {
    pub session: SessionId,
    pub command: Cancellable,
}

impl CancelRequest {
    /// The request `node` holds, or every reason its shape is refused: a `command` that
    /// isn't the name of a command that runs in a session, nor `rebuild`, refused, with
    /// every such name as candidates.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let command = f.word(
                "command",
                &Cancellable::ALL.map(|named| (named.as_str(), named)),
                "commands",
            );
            Some(CancelRequest {
                session: session?,
                command: command?,
            })
        })
    }
}

impl ToWire for CancelRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("command", Json::str(self.command.as_str())),
        ])
    }
}

impl ToWire for Limits {
    /// Every limit, in the design's order (section 6's table).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("document_bytes", Json::count(self.document_bytes)),
            ("request_bytes", Json::count(self.request_bytes)),
            ("request_depth", Json::count(self.request_depth)),
            ("response_bytes", Json::count(self.response_bytes)),
            ("listed", Json::count(self.listed)),
            ("sessions", Json::count(self.sessions)),
            ("published_reads", Json::count(self.published_reads)),
            ("elements", Json::whole(self.elements.into())),
            ("query", Json::whole(self.query.into())),
            ("query_measures", Json::count(self.query_measures)),
            ("highlight", Json::count(self.highlight)),
            ("render_side", Json::whole(self.render_side.into())),
            ("render_seconds", Json::whole(self.render_seconds)),
            ("log_read_events", Json::whole(self.log_read_events.into())),
            ("log_read_bytes", Json::count(self.log_read_bytes)),
            ("log_events", Json::count(self.log_events)),
            ("log_bytes", Json::count(self.log_bytes)),
            (
                "export_tolerance",
                Json::array(self.export_tolerance.map(Json::length)),
            ),
            ("export_triangles", Json::whole(self.export_triangles)),
            ("export_bytes", Json::whole(self.export_bytes)),
            ("kernel_seconds", Json::whole(self.kernel_seconds)),
            ("sketch_seconds", Json::whole(self.sketch_seconds)),
            ("undo_bytes", Json::count(self.undo_bytes)),
            ("disk_check_ms", Json::whole(self.disk_check_ms)),
            ("drag_move_seconds", Json::whole(self.drag_move_seconds)),
            ("drag_unknowns", Json::count(self.drag_unknowns)),
        ])
    }
}

impl ToWire for Version {
    /// `version`'s result: the API's version, the document format's and naming scheme's,
    /// and the limits.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("api", Json::whole(self.api.into())),
            ("format", Json::whole(self.format.into())),
            ("naming", Json::whole(self.naming.into())),
            ("limits", self.limits.to_wire()),
        ])
    }
}

impl ToWire for Opened {
    /// `open`'s result: the path as given (`null` for an untitled document), whether it was
    /// made, the number of features (`null` if the text doesn't read as a document), the
    /// bodies, and the gate's problems.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        Json::object([
            ("path", Json::or_null(self.path.as_deref(), Json::str)),
            ("created", Json::bool(self.created)),
            ("features", Json::or_null(self.features, Json::count)),
            ("bodies", Json::ids(&self.bodies)),
            ("problems", problems),
            ("problems_total", problems_total),
        ])
    }
}

impl ToWire for Closed {
    /// `close`'s result: `{}`.
    fn to_wire(&self) -> Json<'_> {
        Json::object([])
    }
}

impl ToWire for Rebuild {
    /// The revision, what's become of its rebuild, its steps done and steps, the whole
    /// milliseconds it has run (`elapsed_ms`, floored), and each unsuppressed feature's
    /// state, `{"feature", "state"}` in timeline order (each `null` if no rebuild of the
    /// revision has started); not the step running, which the design's `status` doesn't
    /// give.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            ("state", Json::str(self.state.as_str())),
            (
                "done",
                Json::or_null(self.progress.as_ref(), |progress| {
                    Json::count(progress.done)
                }),
            ),
            (
                "steps",
                Json::or_null(self.progress.as_ref(), |progress| {
                    Json::count(progress.steps)
                }),
            ),
            (
                "elapsed_ms",
                Json::or_null(self.progress.as_ref(), |progress| {
                    Json::whole(u64::try_from(progress.elapsed.as_millis()).unwrap_or(u64::MAX))
                }),
            ),
            (
                "features",
                Json::or_null(self.progress.as_ref(), |progress| {
                    Json::array(progress.features.iter().map(|(feature, state)| {
                        Json::object([
                            ("feature", feature.to_wire()),
                            ("state", Json::str(state.as_str())),
                        ])
                    }))
                }),
            ),
        ])
    }
}

impl ToWire for Status {
    /// `status`'s result: the number of features (`null` if the text doesn't read as a
    /// document), whether the gate took it, the transaction open and its undo stack's counts
    /// (`null` if none), what's running on the session's thread (a queued command, or
    /// `reload`; `null` if nothing), the published reads running, the rebuild, the drag open
    /// (`null` if none), and what the last disk comparison found (`null` if none has ended).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("features", Json::or_null(self.features, Json::count)),
            ("gated", Json::bool(self.gated)),
            (
                "transaction",
                Json::or_null(self.transaction.as_ref(), ToWire::to_wire),
            ),
            (
                "running",
                Json::or_null(self.running, |running| Json::str(running.as_str())),
            ),
            (
                "reads",
                Json::array(self.reads.iter().map(|read| Json::str(read.as_str()))),
            ),
            ("rebuild", self.rebuild.to_wire()),
            ("drag", Json::or_null(self.drag.as_ref(), ToWire::to_wire)),
            (
                "disk",
                Json::or_null(self.disk, |disk| Json::str(disk.word())),
            ),
        ])
    }
}

impl ToWire for Cancelled {
    /// `cancel`'s result: the command, and whether it was stopped before it had done its
    /// work.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("command", Json::str(self.command.as_str())),
            ("stopped", Json::bool(self.stopped)),
        ])
    }
}

/// What `cancel` names ([`CANCELLABLE`]), in the design's order: a command that runs in a
/// session, by name, or `rebuild`. Its schema keeps the name `cancel.json` gave it before
/// `rebuild` was added, `SessionCommand`.
pub struct CancellableSchema;

impl JsonSchema for CancellableSchema {
    fn schema_name() -> Cow<'static, str> {
        "SessionCommand".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A command that runs in a session, by name, as a tool is named: what `status` says \
             is running, and `cancel` stops; or `rebuild`, the session's rebuild, while \
             `status.rebuild.state` is `running`, which stops with the command waiting on it.",
            CANCELLABLE,
        )
    }
}

/// What runs on a session's thread, by name, as `status` gives it: a command that runs in a
/// session ([`SESSION_COMMANDS`]), or the session's own reload (docs/design/m5.0-app.md, A3).
pub struct RunningCommandSchema;

impl JsonSchema for RunningCommandSchema {
    fn schema_name() -> Cow<'static, str> {
        "RunningCommand".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "What runs on a session's thread, by name: a command, as a tool is named, which \
             `cancel` stops; or `reload`, the session's own reload of its file, found changed \
             on disk with no transaction open, which nothing cancels.",
            SESSION_COMMANDS
                .iter()
                .copied()
                .chain([RunningCommand::Reload.as_str()]),
        )
    }
}

/// What a comparison of a session's file on disk found, as `status` gives it.
#[derive(JsonSchema)]
#[schemars(rename = "Disk", rename_all = "lowercase")]
pub enum DiskSchema {
    /// The bytes the session last read or wrote.
    Unchanged,
    /// Other bytes. With no transaction open, the session reloads them itself (`reload`);
    /// inside one, its `commit` is refused as a `conflict` until it rolls back. A file this
    /// session's own save left part written reads `changed` too, and isn't reloaded: the
    /// next save says where its old text is.
    Changed,
    /// No file at its path.
    Gone,
    /// Not a regular file this user may read, or past what Silico reads of a document.
    Unreadable,
}

/// A command that runs as a read of the published model, by name ([`PUBLISHED_READS`]), in
/// the design's order.
pub struct PublishedReadSchema;

impl JsonSchema for PublishedReadSchema {
    fn schema_name() -> Cow<'static, str> {
        "PublishedRead".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A command that reads the published model, by name, as a tool is named: what \
             `status` lists among its reads, and `cancel` stops.",
            PUBLISHED_READS,
        )
    }
}

/// `version`'s request: it has no fields. It takes no session, so its response's `session`
/// is null.
#[derive(JsonSchema)]
#[schemars(rename = "VersionRequest", transform = wire_shape)]
pub struct VersionRequestSchema {}

/// The limits every command holds to (the design's section 6), each the value the commands
/// use. A request past one is refused, as `invalid_input`, unless its description says
/// otherwise.
#[derive(JsonSchema)]
#[schemars(rename = "Limits", transform = wire_shape)]
pub struct LimitsSchema {
    /// The most bytes a document's canonical text may be: past it, an edit, a commit, or an
    /// `open` is refused.
    pub document_bytes: u64,
    /// The most bytes a request may be, or a document's file `open` reads.
    pub request_bytes: u64,
    /// How deeply a request's values (or a document's) may nest.
    pub request_depth: u64,
    /// The most bytes a response may be: a command whose result would pass it is refused.
    pub response_bytes: u64,
    /// How many errors (or problems, or warnings) a response lists: the rest are counted,
    /// never refused.
    pub listed: u64,
    /// How many sessions one process holds open at once.
    pub sessions: u64,
    /// How many reads of the published model run at once in one session.
    pub published_reads: u64,
    /// The most entries an `elements` call lists: its `limit` at most.
    pub elements: u64,
    /// The most entries a `query` returns: its `limit` at most.
    pub query: u64,
    /// The most ranges a `query`'s `measures` holds: one a measure.
    pub query_measures: u64,
    /// The most elements one `render` highlights.
    pub highlight: u64,
    /// The most pixels a `render`'s image is across or high.
    pub render_side: u64,
    /// How long one render may take, in seconds: past it, the render worker is stopped, a
    /// `worker_timeout`, which is the machine's fault, never the document's.
    pub render_seconds: u64,
    /// The most events one `read_logs` returns: its `limit` at most.
    pub log_read_events: u64,
    /// The most text one `read_logs` returns, in bytes (its events' messages, spans, and
    /// features): a read stops before the event that would pass it, its `next` saying where.
    pub log_read_bytes: u64,
    /// The most events a session's log buffer keeps: not refused, the oldest are dropped
    /// and counted (`read_logs`' `dropped`).
    pub log_events: u64,
    /// The most text a session's log buffer keeps, in bytes, counted as a read counts it:
    /// not refused, the oldest events are dropped and counted.
    pub log_bytes: u64,
    /// The least and the most tolerance a `3mf` export takes, in millimetres.
    pub export_tolerance: [Length; 2],
    /// The most triangles one `3mf` export writes, every part's in all: past it, refused,
    /// naming `tolerance`.
    pub export_triangles: u64,
    /// The most bytes one `3mf` export's archive may be (a plain zip's, with no ZIP64):
    /// past it, refused, naming `tolerance`.
    pub export_bytes: u64,
    /// How long one request to a kernel worker may take, in seconds: past it, the worker is
    /// stopped, a `worker_timeout`, which is the machine's fault, never the document's.
    pub kernel_seconds: u64,
    /// How long one sketch's solve may take, in seconds: past it, a `worker_timeout`.
    pub sketch_seconds: u64,
    /// The most bytes a transaction's undo stack holds, counted as `status`'s
    /// `transaction.bytes` counts them: not refused, the oldest states after the one the
    /// transaction began from are dropped and counted (`transaction.dropped`), and undo then
    /// stops at the oldest kept; `rollback` still returns to the start.
    pub undo_bytes: u64,
    /// The least time, in milliseconds, from the end of one comparison of a session's file
    /// on disk to the start of the next: not refused. Only `status` starts one, when none is
    /// running and the last ended at least this long ago, so `status`'s `disk`, a
    /// diagnostic, is the last comparison's finding and may be older.
    pub disk_check_ms: u64,
    /// How long one move of a drag may take, in seconds: past it, the drag's worker is
    /// stopped, a `worker_timeout`, and the drag ends. A drag's whole solves (its begin's,
    /// its end's) take a sketch's solve's time.
    pub drag_move_seconds: u64,
    /// The most unknowns the part of a sketch a drag moves may have (two a free point, one a
    /// free radius): past it, `drag_begin` is refused, naming `point`, or `curve`.
    pub drag_unknowns: u64,
}

/// `version`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "VersionResult", transform = wire_shape)]
pub struct VersionResultSchema {
    /// The API's version: ask for it first. One more for each change to a shape that an
    /// older client could misread.
    pub api: u64,
    /// The document format's version, a document's `silico`.
    pub format: u64,
    /// The naming scheme every element's name is in, a document's `naming`.
    pub naming: u64,
    pub limits: LimitsSchema,
}

/// `open`'s request: open a document's file as a session, or make a new, empty one, at a
/// path or, with `path` null, untitled. A document the gate refuses still opens, its
/// problems in the result, so it can be mended with `replace_text`.
#[derive(JsonSchema)]
#[schemars(rename = "OpenRequest", transform = wire_shape)]
pub struct OpenRequestSchema {
    /// The document's path, as every response names the session's: a regular file, if it
    /// isn't to be made. A file this process holds open in another session, by any name, is
    /// refused. Null, with `create` true, for an untitled document: an empty one with no
    /// file, nothing written until a `commit` gives it a path (Save As). Outside a
    /// transaction an edit is saved as it's made, so an untitled document's edits are made
    /// inside one.
    pub path: Option<String>,
    /// Whether to make a new, empty document at the path: refused if anything is there.
    /// Must be true when `path` is null.
    pub create: bool,
}

/// `open`'s result. The response's `session` is the session opened.
#[derive(JsonSchema)]
#[schemars(rename = "OpenResult", transform = wire_shape)]
pub struct OpenResultSchema {
    /// The path, as given: null for an untitled document.
    pub path: Option<String>,
    /// Whether the document was made (`create`).
    pub created: bool,
    /// How many features the document has: null if its text doesn't read as a document.
    pub features: Option<u64>,
    /// The bodies the timeline ends with: the solids no later feature consumed, in timeline
    /// order, found without a kernel.
    pub bodies: Vec<FeatureId>,
    /// The gate's problems with the document, the first 100. A document with any still
    /// opens, and the commands that need a document the gate took refuse until
    /// `replace_text` mends it.
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub problems: Vec<ErrorSchema>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
}

/// `close`'s request: close the session. Refused while a transaction is open: commit or
/// roll back first.
#[derive(JsonSchema)]
#[schemars(rename = "CloseRequest", transform = wire_shape)]
pub struct CloseRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// `close`'s result: it has no fields.
#[derive(JsonSchema)]
#[schemars(rename = "CloseResult", transform = wire_shape)]
pub struct CloseResultSchema {}

/// `status`'s request: what the session holds, answered at once, even while another of its
/// commands runs. It never reads the file.
#[derive(JsonSchema)]
#[schemars(rename = "StatusRequest", transform = wire_shape)]
pub struct StatusRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// What's become of a revision's rebuild.
#[derive(JsonSchema)]
#[schemars(rename = "RebuildState", rename_all = "lowercase")]
pub enum RebuildStateSchema {
    /// Not started: no command has needed its model yet, or the gate refused the document,
    /// so there's nothing to rebuild.
    None,
    /// Running.
    Running,
    /// Done, its model published.
    Done,
    /// Done, but its model holds a kernel fault, kept as the revision's outcome (reads give
    /// it once, in the response's `model.fault`); or its recompute failed.
    Faulted,
    /// Cancelled: the next command that needs its model starts it again.
    Stopped,
}

/// The session's current revision's rebuild.
#[derive(JsonSchema)]
#[schemars(rename = "Rebuild", transform = wire_shape)]
pub struct RebuildSchema {
    /// The revision it's of: the session's current one.
    #[schemars(range(min = 1))]
    pub revision: u64,
    pub state: RebuildStateSchema,
    /// The rebuild's steps settled so far (built, failed, kept from before, or never to
    /// run), all of them once it has finished: progress, a diagnostic, not a result. Null if
    /// no rebuild of the revision has started.
    pub done: Option<u64>,
    /// The steps in the rebuild's plan, none until it's made: progress, a diagnostic. Null
    /// if no rebuild of the revision has started.
    pub steps: Option<u64>,
    /// The whole milliseconds the rebuild has run, from being asked for to its end (its
    /// model published, or its failure or stop), so far while it runs: a diagnostic, a
    /// clock, never a result. Null if no rebuild of the revision has started.
    pub elapsed_ms: Option<u64>,
    /// Each unsuppressed feature's state in the rebuild, in timeline order, listed from its
    /// start and kept once it ends: progress, a diagnostic, not a result (which feature runs
    /// first depends on scheduling). A stopped rebuild leaves what it hadn't settled
    /// `queued`. Null if no rebuild of the revision has started.
    pub features: Option<Vec<FeatureStepSchema>>,
}

/// A feature's state in a rebuild.
#[derive(JsonSchema)]
#[schemars(rename = "FeatureStep", transform = wire_shape)]
pub struct FeatureStepSchema {
    /// The feature's ID.
    pub feature: FeatureId,
    pub state: StepStateSchema,
}

/// What's become of a feature's steps in a rebuild.
#[derive(JsonSchema)]
#[schemars(rename = "StepState", rename_all = "lowercase")]
pub enum StepStateSchema {
    /// Not settled yet: none of its steps running, or the rebuild stopped first (the next
    /// rebuild resumes it).
    Queued,
    /// A step of its is running; a sketch, its solve; a sketch on a face reads queued until
    /// the round that places it, its solve included, then its outcome: its result is the
    /// sketch placed on its face.
    Running,
    /// Built: its solid made, or kept from before; a sketch, solved.
    Built,
    /// Failed on its own: a step the kernel refused, a check its result failed, a sketch
    /// that didn't solve, a pad, pocket, revolve, or groove whose sketch makes no profile,
    /// broken references.
    Failed,
    /// Not built, since a feature it uses failed, faulted, or was blocked.
    Blocked,
    /// Stopped by a kernel fault (a worker that crashed, hung, or couldn't be had); what
    /// uses it is blocked.
    Faulted,
}

/// `status`'s result. What's running is a diagnostic, not a result: it depends on when it's
/// asked.
#[derive(JsonSchema)]
#[schemars(rename = "StatusResult", transform = wire_shape)]
pub struct StatusResultSchema {
    /// How many features the document has: null if its text doesn't read as a document.
    pub features: Option<u64>,
    /// Whether the gate took the document.
    pub gated: bool,
    /// The transaction open, its base as `begin` answered it, with its undo and redo: null
    /// if none is.
    pub transaction: Option<TransactionSchema>,
    /// What's running on the session's thread: a queued command, which `cancel` can stop,
    /// or `reload`, its own reload of a file changed on disk; null if nothing is.
    pub running: Option<RunningCommandSchema>,
    /// Each read of the published model running, in the order they started: `cancel` stops
    /// every one of a name.
    pub reads: Vec<PublishedReadSchema>,
    pub rebuild: RebuildSchema,
    /// The drag open (`drag_begin`): its sketch, and what it grabbed, as the request named
    /// them; null if none is, once it has ended however it ended (`drag_end`, a new revision,
    /// its worker's failure, a cancel of its move). A diagnostic.
    pub drag: Option<super::drag::DragOpenSchema>,
    /// What the last comparison of the session's file on disk found, against the bytes the
    /// session last read or wrote: a diagnostic, never a result, and it reloads nothing
    /// itself. `status` starts the next comparison, off its caller's thread, once the last
    /// ended a second ago or more. Null before one has ended, while the session's own save
    /// runs, and after a save that failed until the next comparison ends.
    pub disk: Option<DiskSchema>,
}

/// `cancel`'s request: stop the command named, if it's running in the session (its queued
/// command, and every read of the published model of that name), or, named `rebuild`, the
/// session's rebuild running, answered at once. A rebuild and the command waiting on it stop
/// together, whichever is named. The command stopped answers too: `cancelled` if it left
/// no trace, or its real result if it had done its work. Refused, naming what is running,
/// if what's named isn't, so a cancel that comes late never stops the command after it.
#[derive(JsonSchema)]
#[schemars(rename = "CancelRequest", transform = wire_shape)]
pub struct CancelRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The command to stop, as `status` names it.
    pub command: CancellableSchema,
}

/// `cancel`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "CancelResult", transform = wire_shape)]
pub struct CancelResultSchema {
    /// The command, as the request named it.
    pub command: CancellableSchema,
    /// Whether it was stopped before it had done its work: false means it had (an edit
    /// already saved), and it answers with its real result. Read `status` for the revision.
    pub stopped: bool,
}
