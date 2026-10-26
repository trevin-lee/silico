//! Silico's structured error (ARCHITECTURE.md, invariant 9). Every failure that reaches a
//! user or an agent is one of these: a stable code to match on, a message, and where they
//! apply, the feature, the sub-elements at fault (every one, or the first of them where
//! there are too many to list, and how many there are), the parameter at fault, the
//! underlying cause, a suggestion, and candidate fixes.
//!
//! The JSON form is the contract with agents (tests/contract/): every field is always
//! present, `null` when absent and `[]` for no elements or candidates, so there's one shape
//! to parse. `elements_total` is how many elements are at fault, `0` for none: more than
//! `elements` lists only when the list is cut short.
//!
//! The elements are the named feature's, with one rule: a pad's, pocket's, revolve's, or
//! groove's refusal for its sketch (no profile, several regions, an axis across it, a slit
//! too thin where it's measured) names the sweep as its feature and the sketch's elements
//! at fault, since that's where they're fixed (docs/design/m3.6-sketch-features.md, S3).

use std::fmt;

use serde::Serialize;

use crate::worker::Advice;

/// Declares the error codes from one list, so the enum, [`ErrorCode::ALL`], and the
/// string forms (which are also the serialized forms) can't drift apart.
macro_rules! error_codes {
    ($($(#[$doc:meta])* $variant:ident = $string:literal,)*) => {
        /// What kind of failure this is. Agents match on the string form, so a code is
        /// never renamed or removed; new codes are appended to the end.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $($(#[$doc])* $variant,)*
        }

        impl ErrorCode {
            /// Every code, in declaration order.
            pub const ALL: &[ErrorCode] = &[$(ErrorCode::$variant,)*];

            /// The stable string form, which is also the serialized form.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant => $string,)*
                }
            }
        }
    };
}

error_codes! {
    /// A request or document value was invalid: out of range, malformed, inconsistent.
    InvalidInput = "invalid_input",
    /// The geometry kernel refused or failed an operation.
    KernelFailed = "kernel_failed",
    /// A worker process that had started (it had said so, as the build Silico started with)
    /// crashed, hung up, or was stopped while handling a request. One that never got that
    /// far is [`ErrorCode::WorkerUnavailable`].
    WorkerCrashed = "worker_crashed",
    /// A worker process didn't reply in time and was stopped.
    WorkerTimeout = "worker_timeout",
    /// A worker's reply couldn't be understood.
    ProtocolError = "protocol_error",
    /// A bug in Silico itself.
    Internal = "internal",
    /// The geometry kernel performed an operation, but its result wasn't valid, so it was
    /// refused rather than built on.
    InvalidResult = "invalid_result",
    /// Silico, or the machine, is short of something the operation needs (open files,
    /// processes, memory, buffers): free some, then try again.
    ResourceExhausted = "resource_exhausted",
    /// A feature rebuilt, but measures otherwise than the fingerprint stored with it: a
    /// determinism bug or a kernel change, for a person to look at, never smoothed over.
    Drift = "drift",
    /// A file couldn't be read or written (missing, not a regular file, not permitted, a
    /// disk full): the client's, not the document's or the kernel's.
    IoFailed = "io_failed",
    /// A reference to an element (a fillet's edge, by its name) names none of the solid
    /// it's of: an upstream edit removed or remade the element. The `candidates` are the
    /// elements it most likely meant, never taken for it.
    ReferenceBroken = "reference_broken",
    /// A sketch's constraints conflict, or some repeat others and disagree with them: the
    /// error's elements are the constraints to remove or change (a shared point is never
    /// among them).
    OverConstrained = "over_constrained",
    /// No solver algorithm could meet a sketch's constraints, starting from where its
    /// geometry is drawn.
    SolveFailed = "solve_failed",
    /// What was asked was made against another state than the session's: a commit refused
    /// because the document's file changed outside the session, an edit made against a
    /// revision the session isn't at, or a cursor of another revision. Rolling back, or
    /// reading again, resolves it; it's neither the document's problem nor the kernel's.
    Conflict = "conflict",
    /// No worker of this build could be had for the request: its executable missing,
    /// unreadable, not runnable, or another build than Silico started with, or its process
    /// ended before its first frame. Silico's install or environment, never the request's or
    /// the document's: no edit helps, and no retry until it's fixed or Silico restarted,
    /// unless the worker was stopped from outside as it started, as its suggestion then says.
    WorkerUnavailable = "worker_unavailable",
    /// A command was stopped before it had done its work, by the client that asked for it
    /// (`cancel`, or MCP's cancellation): it left no trace, and asking again runs it again.
    /// Neither the document's problem nor the kernel's.
    Cancelled = "cancelled",
}

impl ErrorCode {
    /// Whether the code is the kernel's own trouble, which says nothing about what was
    /// asked of it: a worker that crashed, hung, or couldn't be had, a reply that couldn't
    /// be read, a shortage, or a bug in Silico. A command reports one as its own failure (it couldn't
    /// run), not as a problem of the document, and the oracle as a case it couldn't
    /// finish. Every code answers here, so a new code has to say.
    pub fn is_kernel_fault(self) -> bool {
        match self {
            ErrorCode::WorkerCrashed
            | ErrorCode::WorkerUnavailable
            | ErrorCode::WorkerTimeout
            | ErrorCode::ProtocolError
            | ErrorCode::ResourceExhausted
            | ErrorCode::Internal => true,
            ErrorCode::InvalidInput
            | ErrorCode::KernelFailed
            | ErrorCode::InvalidResult
            | ErrorCode::Drift
            | ErrorCode::IoFailed
            | ErrorCode::ReferenceBroken
            | ErrorCode::OverConstrained
            | ErrorCode::SolveFailed
            | ErrorCode::Conflict
            | ErrorCode::Cancelled => false,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structured error. Build one with [`Error::new`] and the `with_*` methods. Boxed, so
/// a `Result` carrying one stays small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(Box<Fields>);

#[derive(Debug, Clone, Serialize)]
struct Fields {
    code: ErrorCode,
    message: String,
    feature: Option<String>,
    elements: Vec<String>,
    /// How many elements are at fault: more than `elements` lists when there are too many
    /// to name, never fewer.
    elements_total: u64,
    parameter: Option<String>,
    cause: Option<String>,
    suggestion: Option<String>,
    candidates: Vec<String>,
    /// What a kernel worker said, as data, that this error was worded from (a measure's
    /// refusal): never written out, and no part of what the error is (two errors are equal
    /// by what they say), but kept by the rebuild cache, so the core that reads it back
    /// words it itself.
    #[serde(skip)]
    worker: Option<Box<WorkerFailure>>,
    /// What `suggestion` says to do, as data, where the worker client worded it from that
    /// ([`Error::with_advice`]): never written out, no part of what the error is, and
    /// dropped by any other suggestion given after.
    #[serde(skip)]
    advice: Option<Box<Advice>>,
}

impl PartialEq for Fields {
    fn eq(&self, other: &Self) -> bool {
        let Fields {
            code,
            message,
            feature,
            elements,
            elements_total,
            parameter,
            cause,
            suggestion,
            candidates,
            worker: _,
            advice: _,
        } = self;
        (
            code,
            message,
            feature,
            elements,
            elements_total,
            parameter,
            cause,
            suggestion,
            candidates,
        ) == (
            &other.code,
            &other.message,
            &other.feature,
            &other.elements,
            &other.elements_total,
            &other.parameter,
            &other.cause,
            &other.suggestion,
            &other.candidates,
        )
    }
}

impl Eq for Fields {}

/// A kernel worker's failure, as data, and the operation it was for, from which the core
/// worded an error.
#[derive(Debug, Clone)]
struct WorkerFailure {
    op: String,
    failure: silico_protocol::Failure,
}

impl Error {
    /// An error with a code and a one-line message saying what went wrong.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(Box::new(Fields {
            code,
            message: message.into(),
            feature: None,
            elements: Vec::new(),
            elements_total: 0,
            parameter: None,
            cause: None,
            suggestion: None,
            candidates: Vec::new(),
            worker: None,
            advice: None,
        }))
    }

    /// The feature the error is about, by name.
    #[must_use]
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.0.feature = Some(feature.into());
        self
    }

    /// The error about no feature: for one that says nothing of the feature it was met in
    /// (a worker that couldn't be had, whichever feature asked for it).
    #[must_use]
    pub(crate) fn without_feature(mut self) -> Self {
        self.0.feature = None;
        self
    }

    /// A sub-element (face, edge, vertex, sketch entity) the error is about, after any
    /// named already.
    #[must_use]
    pub fn with_element(mut self, element: impl Into<String>) -> Self {
        self.0.elements.push(element.into());
        self.count_elements();
        self
    }

    /// Sub-elements the error is about, after any named already.
    #[must_use]
    pub fn with_elements<I, S>(mut self, elements: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.elements.extend(elements.into_iter().map(Into::into));
        self.count_elements();
        self
    }

    /// How many elements are at fault, where `elements` names only the first of them: never
    /// fewer than it names.
    #[must_use]
    pub fn with_elements_total(mut self, total: u64) -> Self {
        self.0.elements_total = total;
        self.count_elements();
        self
    }

    fn count_elements(&mut self) {
        let named = self.0.elements.len() as u64;
        self.0.elements_total = self.0.elements_total.max(named);
    }

    /// The parameter at fault, as a dotted name: the operation's field, and the part of it at
    /// fault where it has parts (`radius`, `offset.x`, a chamfer's `size.angle`).
    #[must_use]
    pub fn with_parameter(mut self, parameter: impl Into<String>) -> Self {
        self.0.parameter = Some(parameter.into());
        self
    }

    /// The error about no parameter: for a client that has no request whose field it could
    /// name (the CLI, whose `path` is the file each of its lines already names).
    #[must_use]
    pub fn without_parameter(mut self) -> Self {
        self.0.parameter = None;
        self
    }

    /// The underlying cause, such as the kernel's own message.
    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.0.cause = Some(cause.into());
        self
    }

    /// What the user or agent could do about it.
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.0.suggestion = Some(suggestion.into());
        self.0.advice = None;
        self
    }

    /// A worker's `advice` as the suggestion, worded as the worker client words it
    /// ([`Advice::words`]), and kept as data, so a client that words it its own way reads
    /// what it says ([`Error::advice`]), never its words.
    #[must_use]
    pub(crate) fn with_advice(mut self, advice: Advice) -> Self {
        self.0.suggestion = Some(advice.words());
        self.0.advice = Some(Box::new(advice));
        self
    }

    /// The worker's advice the suggestion was worded from, if it was.
    pub(crate) fn advice(&self) -> Option<&Advice> {
        self.0.advice.as_deref()
    }

    /// Candidate fixes, such as the elements a broken reference might have meant.
    #[must_use]
    pub fn with_candidates<I, S>(mut self, candidates: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.candidates = candidates.into_iter().map(Into::into).collect();
        self
    }

    /// This error, worded from `failure`, a kernel worker's, for `op`.
    pub(crate) fn with_worker_failure(
        mut self,
        op: &str,
        failure: silico_protocol::Failure,
    ) -> Self {
        self.0.worker = Some(Box::new(WorkerFailure {
            op: op.to_owned(),
            failure,
        }));
        self
    }

    /// The kernel worker's failure this error was worded from, and the operation it was
    /// for, if it was.
    pub(crate) fn worker_failure(&self) -> Option<(&str, &silico_protocol::Failure)> {
        let worker = self.0.worker.as_deref()?;
        Some((&worker.op, &worker.failure))
    }

    pub fn code(&self) -> ErrorCode {
        self.0.code
    }

    pub fn message(&self) -> &str {
        &self.0.message
    }

    pub fn feature(&self) -> Option<&str> {
        self.0.feature.as_deref()
    }

    pub fn elements(&self) -> &[String] {
        &self.0.elements
    }

    /// How many elements are at fault: as many as [`Error::elements`] names, or more when
    /// there are too many to name.
    pub fn elements_total(&self) -> u64 {
        self.0.elements_total
    }

    pub fn parameter(&self) -> Option<&str> {
        self.0.parameter.as_deref()
    }

    pub fn cause(&self) -> Option<&str> {
        self.0.cause.as_deref()
    }

    pub fn suggestion(&self) -> Option<&str> {
        self.0.suggestion.as_deref()
    }

    pub fn candidates(&self) -> &[String] {
        &self.0.candidates
    }
}

/// `code: message (feature F, elements E1, E2, parameter P): cause. Suggestion: S.
/// Candidates: A, B.`, leaving out whatever is absent (`element E` for one; `elements E1, E2,
/// and 3 more` for five, two named).
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.code, self.0.message)?;
        let mut about = Vec::new();
        if let Some(feature) = &self.0.feature {
            about.push(format!("feature {feature}"));
        }
        let unnamed = self
            .0
            .elements_total
            .saturating_sub(self.0.elements.len() as u64);
        match (self.0.elements.as_slice(), unnamed) {
            ([], 0) => {}
            ([element], 0) => about.push(format!("element {element}")),
            (elements, 0) => about.push(format!("elements {}", elements.join(", "))),
            ([], 1) => about.push("1 element".to_owned()),
            ([], unnamed) => about.push(format!("{unnamed} elements")),
            (elements, unnamed) => about.push(format!(
                "elements {}, and {unnamed} more",
                elements.join(", ")
            )),
        }
        if let Some(parameter) = &self.0.parameter {
            about.push(format!("parameter {parameter}"));
        }
        if !about.is_empty() {
            write!(f, " ({})", about.join(", "))?;
        }
        // Text that already ends a sentence doesn't get a second period.
        let sentence = |text: &str| text.strip_suffix('.').unwrap_or(text).to_owned();
        if let Some(cause) = &self.0.cause {
            write!(f, ": {}", sentence(cause))?;
        }
        if let Some(suggestion) = &self.0.suggestion {
            write!(f, ". Suggestion: {}", sentence(suggestion))?;
        }
        if !self.0.candidates.is_empty() {
            write!(f, ". Candidates: {}", self.0.candidates.join(", "))?;
        }
        if self.0.suggestion.is_some() || !self.0.candidates.is_empty() {
            f.write_str(".")?;
        }
        Ok(())
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl std::error::Error for Error {}
