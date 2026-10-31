//! `begin`, `commit`, `rollback`, `format`, `undo`, and `redo`'s wire types
//! (docs/design/m4.1-command-api.md, sections 4 and 7; docs/design/m5.0-app.md, section 7,
//! for `undo` and `redo`): their requests, read strictly ([`SessionRequest`], which `begin`,
//! `commit`, and `rollback` each take, [`FormatRequest`], and [`StepRequest`], which `undo`
//! and `redo` each take); their results, written field for field ([`ToWire`]: a commit's too,
//! which every edit made outside a transaction holds, and the transaction `status` gives);
//! and the mirrors their snapshots, `commands/begin.json`, `commit.json`, `rollback.json`,
//! `format.json`, `undo.json`, and `redo.json`, are generated from. What the values mean (a
//! transaction open or not, a feature the document hasn't, a revision the undo stack
//! doesn't hold) the session says.

use schemars::JsonSchema;

use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    Begun, Committed, Problems, Reformatted, Revision, RolledBack, SessionId, Stepped, Transaction,
};
use crate::document::json::Node;
use crate::document::{FeatureId, PROBLEMS_LISTED};
use crate::{Error, ErrorCode};

/// A request that names only its session: `begin`'s, `commit`'s, and `rollback`'s, and
/// `close`'s, `status`'s, and `outline`'s.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRequest {
    pub session: SessionId,
}

impl SessionRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            Some(SessionRequest { session: session? })
        })
    }
}

impl ToWire for SessionRequest {
    /// The request as a client writes it.
    fn to_wire(&self) -> Json<'_> {
        Json::object([("session", self.session.to_wire())])
    }
}

/// An `undo` or `redo` request: the session, the revision the step was made against, and
/// the revision whose state to restore (`None` for one step).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRequest {
    pub session: SessionId,
    pub revision: Revision,
    pub to: Option<Revision>,
}

impl StepRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let revision = f.revision("revision");
            let to = f.nullable_revision("to");
            Some(StepRequest {
                session: session?,
                revision: revision?,
                to: to?,
            })
        })
    }
}

impl ToWire for StepRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("revision", self.revision.to_wire()),
            ("to", Json::or_null(self.to.as_ref(), ToWire::to_wire)),
        ])
    }
}

/// A `commit` request: the session, the path to save to (`None` for its own file; a path,
/// a Save As, M5.4), and whether a file there may be replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitRequest {
    pub session: SessionId,
    pub path: Option<String>,
    pub replace: bool,
}

impl CommitRequest {
    /// The request `node` holds, or every reason its shape is refused: as well as each
    /// field's own, `replace` true with no `path` (it saves to its own file, which is
    /// always written over), as fields that must agree are refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let path = f.nullable_string("path");
            let replace = f.bool("replace");
            let (path, replace) = (path?, replace?);
            if path.is_none() && replace {
                f.refuse(
                    Error::new(
                        ErrorCode::InvalidInput,
                        "replace must be false for a commit to the session's own file (path null)",
                    )
                    .with_parameter("replace")
                    .with_suggestion(
                        "give replace false to save to the session's own file, or a path to save elsewhere",
                    ),
                );
                return None;
            }
            Some(CommitRequest {
                session: session?,
                path: path.map(str::to_owned),
                replace,
            })
        })
    }
}

impl ToWire for CommitRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("path", Json::or_null(self.path.as_deref(), Json::str)),
            ("replace", Json::bool(self.replace)),
        ])
    }
}

/// A `format` request: the session, the features whose drift it accepts (`None` for every
/// feature's), and whether it writes the file.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatRequest {
    pub session: SessionId,
    pub features: Option<Vec<FeatureId>>,
    pub write: bool,
}

impl FormatRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let features = f.nullable_feature_ids("features");
            let write = f.bool("write");
            Some(FormatRequest {
                session: session?,
                features: features?,
                write: write?,
            })
        })
    }
}

impl ToWire for FormatRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            (
                "features",
                Json::or_null(self.features.as_deref(), Json::ids),
            ),
            ("write", Json::bool(self.write)),
        ])
    }
}

impl ToWire for Begun {
    /// `begin`'s result: the revision the transaction began from.
    fn to_wire(&self) -> Json<'_> {
        Json::object([("base", self.base.to_wire())])
    }
}

impl ToWire for Transaction {
    /// The transaction open, as `status` gives it: the revision it began from, whether its
    /// document differs from that one's, the edits left to undo and to redo, those the
    /// stack's bound put out of undo's reach, and what the stack holds.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("base", self.base.to_wire()),
            ("changed", Json::bool(self.changed)),
            ("undo", Json::count(self.undo)),
            ("redo", Json::count(self.redo)),
            ("dropped", Json::count(self.dropped)),
            ("bytes", Json::count(self.bytes)),
        ])
    }
}

impl ToWire for Stepped {
    /// `undo`'s and `redo`'s result: the revision whose state is restored, the edits left to
    /// undo and to redo, and how many more the step put out of undo's reach.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("restored", self.restored.to_wire()),
            ("undo", Json::count(self.undo)),
            ("redo", Json::count(self.redo)),
            ("dropped", Json::count(self.dropped)),
        ])
    }
}

impl ToWire for Committed {
    /// `commit`'s result, and the commit an edit outside a transaction holds: whether the
    /// file was written, its path, the features refingerprinted and the sketches re-solved,
    /// and the problems it couldn't fix.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        Json::object([
            ("written", Json::bool(self.written)),
            ("path", Json::str(&self.path)),
            ("refingerprinted", Json::ids(&self.refingerprinted)),
            ("resolved", Json::ids(&self.resolved)),
            ("problems", problems),
            ("problems_total", problems_total),
        ])
    }
}

impl ToWire for RolledBack {
    /// `rollback`'s result: the revision whose state it restored.
    fn to_wire(&self) -> Json<'_> {
        Json::object([("restored", self.restored.to_wire())])
    }
}

impl ToWire for Reformatted {
    /// `format`'s result: whether the canonical text differs from the file and was written,
    /// the path, the features refingerprinted, the sketches re-solved, the drift accepted,
    /// and the problems it couldn't fix.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        Json::object([
            ("changed", Json::bool(self.changed)),
            ("written", Json::bool(self.written)),
            ("path", Json::str(&self.path)),
            ("refingerprinted", Json::ids(&self.refingerprinted)),
            ("resolved", Json::ids(&self.resolved)),
            ("accepted", Json::ids(&self.accepted)),
            ("problems", problems),
            ("problems_total", problems_total),
        ])
    }
}

/// `begin`'s request: open a transaction in the session. Inside it, edits change only what
/// the session holds, until `commit` saves them or `rollback` undoes them all.
#[derive(JsonSchema)]
#[schemars(rename = "BeginRequest", transform = wire_shape)]
pub struct BeginRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// `begin`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "BeginResult", transform = wire_shape)]
pub struct BeginResultSchema {
    /// The revision the transaction began from: the one `rollback` returns to.
    #[schemars(range(min = 1))]
    pub base: u64,
}

/// The transaction open in a session, as `status` gives it.
#[derive(JsonSchema)]
#[schemars(rename = "Transaction", transform = wire_shape)]
pub struct TransactionSchema {
    /// The revision the transaction began from: the one `rollback` returns to.
    #[schemars(range(min = 1))]
    pub base: u64,
    /// Whether its document differs from the one it began from, as a commit compares them:
    /// unsaved changes. False after undoing back to that one, or editing back to it, and a
    /// commit then writes nothing.
    pub changed: bool,
    /// How many of its edits `undo` can undo, one step each.
    pub undo: u64,
    /// How many `redo` can redo: the edits undone, until an edit drops them.
    pub redo: u64,
    /// How many of its edits undo can no longer reach, the undo stack's bound (`version`'s
    /// `undo_bytes`) having dropped their states: 2 at the first drop (the edits before and
    /// after the state dropped), one more at each after. `rollback` still returns to the
    /// state it began from.
    pub dropped: u64,
    /// What its undo stack holds, in bytes, as its bound counts it: the file's bytes as it
    /// began, and each earlier state's texts beyond what they share.
    pub bytes: u64,
}

/// `commit`'s request: end the session's transaction, saving its document to its own
/// file, or, given a path, to the file there (Save As), which the session then holds.
#[derive(JsonSchema)]
#[schemars(rename = "CommitRequest", transform = wire_shape)]
pub struct CommitRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// null to save to the session's own file. A path (relative to the workspace's root
    /// under `silico serve`) to save to that file instead, written even if nothing changed:
    /// it must end in .silico, can't be the session's own file or one another session
    /// holds, and the session holds it from then on, letting its old file go.
    pub path: Option<String>,
    /// Whether to replace a file already at `path`. Must be false when `path` is null.
    pub replace: bool,
}

/// What a commit did: `commit`'s result, and the commit an edit made outside a transaction
/// holds (each such edit is saved as its own transaction).
#[derive(JsonSchema)]
#[schemars(rename = "CommitResult", transform = wire_shape)]
pub struct CommitResultSchema {
    /// Whether the file was written: false when the document is the one the transaction
    /// began with.
    pub written: bool,
    /// The document's path, as `open` was given it, or, after a Save As, as the commit was.
    pub path: String,
    /// The features the edits reached whose fingerprints were written anew, in timeline
    /// order.
    pub refingerprinted: Vec<FeatureId>,
    /// The sketches the edits reached whose solves were written anew, in timeline order.
    pub resolved: Vec<FeatureId>,
    /// What the commit couldn't fix, the first 100: failed features, and the drift of
    /// features no edit reached, whose fingerprints it leaves as stored (only `format`
    /// accepts drift).
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub problems: Vec<ErrorSchema>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
}

/// `rollback`'s request: end the session's transaction, undoing every edit made in it.
#[derive(JsonSchema)]
#[schemars(rename = "RollbackRequest", transform = wire_shape)]
pub struct RollbackRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// `rollback`'s result. The session is at a new revision, whose state equals the restored
/// one's.
#[derive(JsonSchema)]
#[schemars(rename = "RollbackResult", transform = wire_shape)]
pub struct RollbackResultSchema {
    /// The revision whose state is restored: the one the transaction began from.
    #[schemars(range(min = 1))]
    pub restored: u64,
}

/// `format`'s request: save the document in canonical form with every feature's
/// fingerprint as rebuilt, as `silico fmt` does, accepting the drift of the features named.
/// Inside a transaction it writes no file: what it would write becomes the transaction's
/// document, one edit `undo` can undo, which the transaction's `commit` saves.
#[derive(JsonSchema)]
#[schemars(rename = "FormatRequest", transform = wire_shape)]
pub struct FormatRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The features whose drift to accept, by ID, or null for every feature's. Any other
    /// drifted feature keeps its stored fingerprint and is a problem.
    pub features: Option<Vec<FeatureId>>,
    /// Whether to write the file (inside a transaction, its document): false only says
    /// whether it would change (`silico fmt --check`), changing nothing.
    pub write: bool,
}

/// `format`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "FormatResult", transform = wire_shape)]
pub struct FormatResultSchema {
    /// Whether the canonical text differs from the file; inside a transaction, whether the
    /// canonical text differs from the transaction's.
    pub changed: bool,
    /// Whether the file was written: never inside a transaction.
    pub written: bool,
    /// The document's path, as `open` was given it.
    pub path: String,
    /// Every feature whose fingerprint was written (or, not writing, would be), in timeline
    /// order.
    pub refingerprinted: Vec<FeatureId>,
    /// Every sketch whose solve was written (or would be), in timeline order.
    pub resolved: Vec<FeatureId>,
    /// The features among those whose drift was accepted: a stored fingerprint the rebuild
    /// didn't match.
    pub accepted: Vec<FeatureId>,
    /// What format couldn't fix, the first 100: failed features, which keep their
    /// fingerprints, and drift it didn't accept.
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub problems: Vec<ErrorSchema>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
}

/// `undo`'s request: inside a transaction, the state before its last edit not undone
/// restored, or with `to`, the state at that revision, undoing every edit after it; at a new
/// revision, whose state equals that one, byte for byte. Never touches the file.
#[derive(JsonSchema)]
#[schemars(rename = "UndoRequest", transform = wire_shape)]
pub struct UndoRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the undo was made against: one against another is refused
    /// as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// Null to undo one edit; or an earlier revision whose state the undo stack holds, to
    /// undo every edit after it, as a dialog's Cancel undoes to the revision it opened at.
    /// The session's own revision, a later one, or one the stack doesn't hold (from before
    /// the transaction, past its bound, or dropped by an edit after an undo) is refused,
    /// the revisions it holds its candidates.
    #[schemars(range(min = 1))]
    pub to: Option<u64>,
}

/// `undo`'s result. The session is at a new revision, whose state equals the restored one's.
#[derive(JsonSchema)]
#[schemars(rename = "UndoResult", transform = wire_shape)]
pub struct UndoResultSchema {
    /// The revision whose state is restored.
    #[schemars(range(min = 1))]
    pub restored: u64,
    /// How many edits are left to undo.
    pub undo: u64,
    /// How many edits are left to redo.
    pub redo: u64,
    /// How many more of the transaction's edits this step put out of undo's reach, the
    /// stack's bound having dropped its oldest states (a state restored can hold more than
    /// the one it replaces): 2 at the first drop, one more at each after, as `status`'s
    /// `transaction.dropped` counts them; 0 if it dropped none.
    pub dropped: u64,
}

/// `redo`'s request: inside a transaction, the state after its last edit undone restored,
/// or with `to`, the state at that revision, redoing every edit up to it; at a new revision,
/// whose state equals that one, byte for byte. Never touches the file.
#[derive(JsonSchema)]
#[schemars(rename = "RedoRequest", transform = wire_shape)]
pub struct RedoRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The session's revision the redo was made against: one against another is refused
    /// as a `conflict`.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// Null to redo one edit; or a later revision whose state the undo stack holds, to redo
    /// every edit up to it. The session's own revision, an earlier one, or one the stack
    /// doesn't hold (an edit after an undo drops what was undone) is refused, the revisions
    /// it holds its candidates.
    #[schemars(range(min = 1))]
    pub to: Option<u64>,
}

/// `redo`'s result. The session is at a new revision, whose state equals the restored one's.
#[derive(JsonSchema)]
#[schemars(rename = "RedoResult", transform = wire_shape)]
pub struct RedoResultSchema {
    /// The revision whose state is restored.
    #[schemars(range(min = 1))]
    pub restored: u64,
    /// How many edits are left to undo.
    pub undo: u64,
    /// How many edits are left to redo.
    pub redo: u64,
    /// How many more of the transaction's edits this step put out of undo's reach, the
    /// stack's bound having dropped its oldest states: 2 at the first drop, one more at each
    /// after, as `status`'s `transaction.dropped` counts them; 0 if it dropped none.
    pub dropped: u64,
}
