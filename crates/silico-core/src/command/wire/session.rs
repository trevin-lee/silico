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
use super::transaction::BeginResultSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape, words};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    Cancelled, Closed, Limits, Opened, Problems, Rebuild, SessionCommand, SessionId, Status,
    Version,
};
use crate::document::json::Node;
use crate::document::{FeatureId, PROBLEMS_LISTED};

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

/// An `open` request: the document's path, UTF-8 (a response names it exactly), and whether
/// to make a new, empty document there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: String,
    pub create: bool,
}

impl OpenRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let path = f.string("path");
            let create = f.bool("create");
            Some(OpenRequest {
                path: path?.to_owned(),
                create: create?,
            })
        })
    }
}

impl ToWire for OpenRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("path", Json::str(&self.path)),
            ("create", Json::bool(self.create)),
        ])
    }
}

/// The commands that run in a session, each by its name, in the design's order (a subset of
/// [`COMMANDS`]): what `status` says is running, and `cancel` stops ([`SessionCommand::ALL`]).
pub const SESSION_COMMANDS: [&str; SessionCommand::ALL.len()] = {
    let mut names = [""; SessionCommand::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = SessionCommand::ALL[at].as_str();
        at += 1;
    }
    names
};

/// How many commands run as reads that may read the published model.
const PUBLISHED_READ_COUNT: usize = {
    let mut count = 0;
    let mut at = 0;
    while at < SessionCommand::ALL.len() {
        if SessionCommand::ALL[at].reads_published() {
            count += 1;
        }
        at += 1;
    }
    count
};

/// The commands that run as reads that may read the published model
/// ([`SessionCommand::reads_published`]), each by name, in the design's order (a subset of
/// [`SESSION_COMMANDS`]): what `status` lists among the published reads running.
pub const PUBLISHED_READS: [&str; PUBLISHED_READ_COUNT] = {
    let mut names = [""; PUBLISHED_READ_COUNT];
    let (mut at, mut read) = (0, 0);
    while at < SessionCommand::ALL.len() {
        if SessionCommand::ALL[at].reads_published() {
            names[read] = SessionCommand::ALL[at].as_str();
            read += 1;
        }
        at += 1;
    }
    names
};

/// A `cancel` request: the session, and the command to stop, by its name: one of the
/// commands that run in a session ([`SESSION_COMMANDS`]), never any other string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelRequest {
    pub session: SessionId,
    pub command: SessionCommand,
}

impl CancelRequest {
    /// The request `node` holds, or every reason its shape is refused: a `command` that
    /// isn't the name of a command that runs in a session refused, with every such name as
    /// candidates.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let command = f.word(
                "command",
                &SessionCommand::ALL.map(|command| (command.as_str(), command)),
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
            ("kernel_seconds", Json::whole(self.kernel_seconds)),
            ("sketch_seconds", Json::whole(self.sketch_seconds)),
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
    /// `open`'s result: the path as given, whether it was made, the number of features
    /// (`null` if the text doesn't read as a document), the bodies, and the gate's problems.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        Json::object([
            ("path", Json::str(&self.path)),
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
    /// The revision, and what's become of its rebuild.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            ("state", Json::str(self.state.as_str())),
        ])
    }
}

impl ToWire for Status {
    /// `status`'s result: the number of features (`null` if the text doesn't read as a
    /// document), whether the gate took it, the transaction open (`null` if none), the
    /// queued command running (`null` if none), the published reads running, and the
    /// rebuild.
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
                Json::or_null(self.running, |command| Json::str(command.as_str())),
            ),
            (
                "reads",
                Json::array(self.reads.iter().map(|read| Json::str(read.as_str()))),
            ),
            ("rebuild", self.rebuild.to_wire()),
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

/// A command that runs in a session, by name ([`SESSION_COMMANDS`]), in the design's order.
pub struct SessionCommandSchema;

impl JsonSchema for SessionCommandSchema {
    fn schema_name() -> Cow<'static, str> {
        "SessionCommand".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A command that runs in a session, by name, as a tool is named: what `status` says \
             is running, and `cancel` stops.",
            SESSION_COMMANDS,
        )
    }
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
    /// How long one request to a kernel worker may take, in seconds: past it, the worker is
    /// stopped, a `worker_timeout`, which is the machine's fault, never the document's.
    pub kernel_seconds: u64,
    /// How long one sketch's solve may take, in seconds: past it, a `worker_timeout`.
    pub sketch_seconds: u64,
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

/// `open`'s request: open a document's file as a session, or make a new, empty one. A
/// document the gate refuses still opens, its problems in the result, so it can be mended
/// with `replace_text`.
#[derive(JsonSchema)]
#[schemars(rename = "OpenRequest", transform = wire_shape)]
pub struct OpenRequestSchema {
    /// The document's path, as every response names the session's: a regular file, if it
    /// isn't to be made. A file this process holds open in another session, by any name, is
    /// refused.
    pub path: String,
    /// Whether to make a new, empty document at the path: refused if anything is there.
    pub create: bool,
}

/// `open`'s result. The response's `session` is the session opened.
#[derive(JsonSchema)]
#[schemars(rename = "OpenResult", transform = wire_shape)]
pub struct OpenResultSchema {
    /// The path, as given.
    pub path: String,
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
    /// The transaction open, as `begin` answered it: null if none is.
    pub transaction: Option<BeginResultSchema>,
    /// The session's queued command running, which `cancel` can stop: null if none is.
    pub running: Option<SessionCommandSchema>,
    /// Each read of the published model running, in the order they started: `cancel` stops
    /// every one of a name.
    pub reads: Vec<PublishedReadSchema>,
    pub rebuild: RebuildSchema,
}

/// `cancel`'s request: stop the command named, if it's running in the session (its queued
/// command, and every read of the published model of that name), answered at once. The
/// command stopped answers too: `cancelled` if it left no trace, or its real result if it
/// had done its work. Refused, naming what is running, if that command isn't, so a cancel
/// that comes late never stops the command after it.
#[derive(JsonSchema)]
#[schemars(rename = "CancelRequest", transform = wire_shape)]
pub struct CancelRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The command to stop, as `status` names it.
    pub command: SessionCommandSchema,
}

/// `cancel`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "CancelResult", transform = wire_shape)]
pub struct CancelResultSchema {
    /// The command, as the request named it.
    pub command: SessionCommandSchema,
    /// Whether it was stopped before it had done its work: false means it had (an edit
    /// already saved), and it answers with its real result. Read `status` for the revision.
    pub stopped: bool,
}
