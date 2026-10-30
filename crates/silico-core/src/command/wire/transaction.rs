//! `begin`, `commit`, `rollback`, and `format`'s wire types (docs/design/m4.1-command-api.md,
//! sections 4 and 7): their requests, read strictly ([`SessionRequest`], which `begin`,
//! `commit`, and `rollback` each take, and [`FormatRequest`]); their results, written field
//! for field ([`ToWire`]: a commit's too, which every edit made outside a transaction holds);
//! and the mirrors their snapshots, `commands/begin.json`, `commit.json`, `rollback.json`,
//! and `format.json`, are generated from. What the values mean (a transaction open or not, a
//! feature the document hasn't) the session says.

use schemars::JsonSchema;

use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{Begun, Committed, Problems, Reformatted, RolledBack, SessionId};
use crate::document::json::Node;
use crate::document::{FeatureId, PROBLEMS_LISTED};

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

/// `commit`'s request: end the session's transaction, saving its document.
#[derive(JsonSchema)]
#[schemars(rename = "CommitRequest", transform = wire_shape)]
pub struct CommitRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// What a commit did: `commit`'s result, and the commit an edit made outside a transaction
/// holds (each such edit is saved as its own transaction).
#[derive(JsonSchema)]
#[schemars(rename = "CommitResult", transform = wire_shape)]
pub struct CommitResultSchema {
    /// Whether the file was written: false when the document is the one the transaction
    /// began with.
    pub written: bool,
    /// The document's path, as `open` was given it.
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
