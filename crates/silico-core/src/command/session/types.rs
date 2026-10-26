//! A session's IDs and revisions, what its responses say of it and of the model read, the
//! handle a command answers through ([`Running`]), and each command's result.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

use super::Problems;
#[cfg(doc)]
use super::Sessions;
#[cfg(doc)]
use crate::document;
use crate::document::json::Plain;
use crate::document::{Feature, FeatureId};
use crate::recompute::Model;
use crate::{Error, ErrorCode};

/// A session's ID as a request's schema gives it, the IDs [`SessionId::parse`] reads: `s`
/// and a whole number from 1, with no leading zeros.
pub(crate) const SESSION_ID_PATTERN: &str = r"^s[1-9][0-9]*$";

/// A session's ID, written `s1`, `s2`, ...: the process's count of opens, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub(super) u64);

impl SessionId {
    /// Its number: 1 for the process's first open, and one more for each after it.
    pub const fn number(self) -> u64 {
        self.0
    }

    /// The ID `text` writes, `s1`, `s2`, ...: `s` and a whole number from 1, with no
    /// leading zeros; none for anything else. A request names its session so
    /// (`command::wire`).
    pub fn parse(text: &str) -> Option<SessionId> {
        crate::command::whole_number(text.strip_prefix('s')?)
            .filter(|&n| n >= 1)
            .map(SessionId)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

/// Which state of its document a session is at: 1 at `open`, and one more for each change
/// of the document. It never goes back and is never reused, so a result and an edit can say
/// which state they're of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub(super) u64);

impl Revision {
    /// A session's revision when it opens.
    pub const OPENED: Revision = Revision(1);

    /// The revision after this one: each change to a session's document gives one more
    /// (design §3); none past `u64::MAX`, so a revision is never reused (an edit then is
    /// refused, as an open is when session IDs run out).
    #[must_use]
    pub const fn next(self) -> Option<Revision> {
        match self.0.checked_add(1) {
            Some(next) => Some(Revision(next)),
            None => None,
        }
    }

    /// Its number, as a response writes it: 1 at `open`, then one more for each change.
    pub const fn number(self) -> u64 {
        self.0
    }

    /// The revision numbered `number`, as a request names one: none for 0, which no
    /// revision is.
    pub const fn from_number(number: u64) -> Option<Revision> {
        if number == 0 {
            None
        } else {
            Some(Revision(number))
        }
    }
}

/// A session as a command left it: what every response says of it (the envelope's
/// `session`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: SessionId,
    /// Its document's path as `open` was given it.
    pub path: String,
    pub revision: Revision,
    /// Whether a transaction is open (the design's section 4).
    pub transaction: bool,
    /// Whether the session reloaded its file before the command, having found it changed
    /// outside the session (decision 2): never inside a transaction, nor for `close`,
    /// `status`, and published reads, which don't read the file.
    pub reloaded: bool,
}

/// A command's answer: the session as the command left it (none for an `open` that failed,
/// or a session that isn't open), the model it read, if it read one, and the command's
/// result, or every reason it was refused, in the order found, the first
/// [`document::PROBLEMS_LISTED`] listed and all counted.
#[derive(Debug, Clone, PartialEq)]
pub struct Response<T> {
    pub session: Option<SessionInfo>,
    pub model: Option<ModelRead>,
    pub result: Result<T, Problems>,
}

/// The model a command read (the envelope's `model`, design §2): the revision it was rebuilt
/// from, and whether it was read as the current revision's or as the newest published (an
/// older revision's, maybe). The fault its rebuild met comes with decision 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRead {
    pub revision: Revision,
    pub of: ModelChoice,
}

/// What a read given to [`Sessions::read_model`] is handed: the model, and what was read.
/// The published read lane's kernel client and a check of whether the read was stopped
/// (decisions 33 and 34) come later, here, without changing a read's signature.
pub struct Read<'a> {
    pub(super) model: &'a Model,
    pub(super) read: ModelRead,
}

impl Read<'_> {
    /// The model read.
    pub fn model(&self) -> &Model {
        self.model
    }

    /// Which model it is: the response's `model`.
    pub fn model_read(&self) -> ModelRead {
        self.read
    }
}

/// Which model a read takes (decision 33): the current revision's, waited for, or the
/// newest published, never waiting or queueing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    Current,
    Published,
}

impl ModelChoice {
    /// Every choice, in the order a refusal lists them.
    pub const ALL: [ModelChoice; 2] = [ModelChoice::Current, ModelChoice::Published];

    /// As a request and a response write it: `current` or `published`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ModelChoice::Current => "current",
            ModelChoice::Published => "published",
        }
    }
}

/// The newest model a session's rebuilds published, and the revision it's the model of (the
/// same as its response's `model.revision`): the revision it was rebuilt from, or, after a
/// rollback, the new revision whose state is the one it was rebuilt from (the design's
/// section 4). What [`Sessions::published`] gives the app to draw, never waiting for a
/// rebuild.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub revision: Revision,
    pub model: Arc<Model>,
}

/// A command running on its session's thread: wait for its answer, poll it, or drop it
/// (the command runs on, its answer unread).
#[must_use = "a command's answer is only had by waiting for it"]
pub struct Running<T> {
    answer: Receiver<Response<T>>,
    /// What's said if the thread answering ends without an answer (a bug): which stopped.
    lost: &'static str,
}

/// A queued command's thread ended without answering.
pub(super) const SESSION_STOPPED: &str = "the session stopped before the command answered";

/// A published read's thread ended without answering (its read panicked).
pub(super) const READ_STOPPED: &str = "the read stopped before it answered";

impl<T> Running<T> {
    pub(super) fn new(answer: Receiver<Response<T>>, lost: &'static str) -> Self {
        Running { answer, lost }
    }

    /// A command answered before it was queued (refused: no such session, too many open).
    pub(super) fn answered(response: Response<T>) -> Self {
        let (reply, answer) = mpsc::sync_channel(1);
        let _ = reply.send(response);
        Running::new(answer, SESSION_STOPPED)
    }

    /// Waits for the answer.
    pub fn wait(self) -> Response<T> {
        let lost = self.lost;
        self.answer.recv().unwrap_or_else(|_| unanswered(lost))
    }

    /// Waits up to `timeout` for the answer, or gives the handle back to wait on again.
    pub fn wait_timeout(self, timeout: Duration) -> Result<Response<T>, Self> {
        match self.answer.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(RecvTimeoutError::Timeout) => Err(self),
            Err(RecvTimeoutError::Disconnected) => Ok(unanswered(self.lost)),
        }
    }

    /// The answer, if it has come, or the handle back to wait on.
    pub fn try_take(self) -> Result<Response<T>, Self> {
        match self.answer.try_recv() {
            Ok(response) => Ok(response),
            Err(TryRecvError::Empty) => Err(self),
            Err(TryRecvError::Disconnected) => Ok(unanswered(self.lost)),
        }
    }
}

/// The answer of a command whose thread ended without giving one (a bug): `lost` says which
/// stopped.
fn unanswered<T>(lost: &str) -> Response<T> {
    Response {
        session: None,
        model: None,
        result: Err(Problems::from(
            Error::new(ErrorCode::Internal, lost).with_suggestion(crate::worker::SILICO_BUG),
        )),
    }
}

/// What `open` found: the path as given, whether it made the file, how many features the
/// document has (none if its text doesn't read as a document), the bodies its timeline ends
/// with (as `silico show` finds them, needing no kernel), and the gate's problems, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub path: String,
    pub created: bool,
    pub features: Option<usize>,
    pub bodies: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `begin` answers: the revision the transaction began from, which `rollback` returns
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Begun {
    pub base: Revision,
}

/// What `commit` did (the design's section 7): whether it wrote the file (not if the
/// document is the one `begin` saw), its path as `open` was given it, the features the
/// transaction's edits reached whose fingerprints it wrote anew and the reached sketches
/// whose solves it wrote, in timeline order (decision 28), and the problems it can't fix:
/// failed features, solids that couldn't be measured, and the drift of features no edit
/// reached, which keep what's stored with them.
#[derive(Debug, Clone, PartialEq)]
pub struct Committed {
    pub written: bool,
    pub path: String,
    pub refingerprinted: Vec<FeatureId>,
    pub resolved: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `rollback` answers: the revision whose state it restored. The session is at a new
/// revision, whose state equals that one's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolledBack {
    pub restored: Revision,
}

/// What `format` did (decision 31): whether its canonical text differs from the file (as
/// the session read it before the command, reloading a change, decision 2), whether it
/// wrote it, the path as `open` was given it, the features whose fingerprints it wrote (or,
/// not writing, would write) anew and the sketches whose solves, the features among them
/// whose drift it accepted, each in timeline order, and what it can't fix: failed features,
/// solids that couldn't be measured, and the drift it didn't accept.
#[derive(Debug, Clone, PartialEq)]
pub struct Reformatted {
    pub changed: bool,
    pub written: bool,
    pub path: String,
    pub refingerprinted: Vec<FeatureId>,
    pub resolved: Vec<FeatureId>,
    pub accepted: Vec<FeatureId>,
    pub problems: Problems,
}

/// What `add_feature` did (the design's section 7): the feature as the document now holds it
/// (its ID, the one given or the one made, and its version; outside a transaction, as its
/// commit saved it, with the fingerprint written), whether the document changed (always, a
/// feature added), and, outside a transaction, the commit's result, none inside one.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub feature: Feature,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `set_field` did (the design's section 7): the feature and the field, as the request
/// named them, the field's value before and after, each as the document's canonical form
/// writes it (`null` for a field the feature hasn't, a note removed; after, outside a
/// transaction, as its commit saved it, a sketch's solve and all), whether the document
/// changed (not if the field was given the value it has: then there's no new revision and
/// nothing is written), and, outside a transaction, the commit's result, none inside one or
/// if nothing changed.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSet {
    pub feature: FeatureId,
    pub field: String,
    pub before: Plain,
    pub after: Plain,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `remove_feature` did (the design's section 7): the feature removed, as the document
/// held it, its place in the timeline, from 0, so `replace_text` can put it back, whether the
/// document changed (always), and, outside a transaction, the commit's result, none inside
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    pub feature: Feature,
    pub index: usize,
    pub changed: bool,
    pub commit: Option<Committed>,
}

/// What `replace_text` did (the design's section 7): whether the document changed (not if
/// the text is the document the session holds, in whatever layout: then there's no new
/// revision and nothing is written), its number of features, and, outside a transaction,
/// the text it replaced (so a client can put it back) and the commit's result (none if
/// nothing changed); inside one, neither (a rollback restores the text).
#[derive(Debug, Clone, PartialEq)]
pub struct Replaced {
    pub changed: bool,
    pub features: usize,
    pub before: Option<String>,
    pub commit: Option<Committed>,
}

/// What `status` says of a session, from what it holds: how many features its document has
/// (none if its text doesn't read as one), whether the gate took it, and the transaction
/// open, if one is: the revision it began from, which `rollback` returns to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub features: Option<usize>,
    pub gated: bool,
    pub transaction: Option<Begun>,
}
