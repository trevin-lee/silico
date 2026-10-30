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

/// Declares a list of codes, so the enum, its `ALL`, and the string forms (which are also
/// the serialized forms) can't drift apart: the error codes ([`ErrorCode`]) and the warning
/// codes ([`WarningCode`]), each a list of its own, and the words of a worker's advice
/// (`worker::AdviceWord`).
macro_rules! codes {
    ($(#[$enum_doc:meta])* $name:ident { $($(#[$doc:meta])* $variant:ident = $string:literal,)* }) => {
        $(#[$enum_doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum $name {
            $($(#[$doc])* $variant,)*
        }

        impl $name {
            /// Every code, in declaration order.
            pub const ALL: &[$name] = &[$($name::$variant,)*];

            /// The stable string form, which is also the serialized form.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $string,)*
                }
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
pub(crate) use codes;

codes! {
    /// What kind of failure this is. Agents match on the string form, so a code is never
    /// renamed or removed; new codes are appended to the end.
    ErrorCode {
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
}

codes! {
    /// What a warning is about (docs/design/m4.1-command-api.md, decision 17): something
    /// `check` finds that fails nothing (no command refused, no save blocked, no exit code)
    /// but is worth saying. A list of its own, beside the error codes, with the same rule:
    /// agents match on the string form, so a code is never renamed or removed; new codes are
    /// appended to the end (`tests/contract/warning-codes.txt`).
    WarningCode {
        /// Two curve ends of a sketch's profile geometry are at one place, within FreeCAD's
        /// joining distance (closer than 1e-7 mm), but aren't one point, so the profile is open
        /// there: FreeCAD would join them, Silico joins curves only at the points they share
        /// (docs/design/m3.2-sketch-format.md, D2).
        EndsNotJoined = "ends_not_joined",
        /// Constraints of a sketch repeat what its others already fix, and agree with them
        /// (docs/design/m3.3-solving.md, decision 5): the sketch solves, as SolidWorks solves it,
        /// where FreeCAD calls it a sketch with redundant constraints.
        ConstraintsRedundant = "constraints_redundant",
        /// A feature built, but its file stores no fingerprint, so a change in what it builds
        /// isn't caught as drift (the maintainer, 2026-10-26).
        FingerprintMissing = "fingerprint_missing",
        /// Constraints of a sketch only part of whose equations repeat what its others already
        /// fix, and that part agrees with them (docs/design/m3.3-solving.md, decision 5): the
        /// sketch solves, where FreeCAD calls them partially redundant constraints. Apart from
        /// `constraints_redundant`, whose advice is to remove them: removing one of these frees
        /// what only it fixes (approved at M4's close, 2026-10-29).
        ConstraintsPartiallyRedundant = "constraints_partially_redundant",
    }
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

/// A structured error. Build one with [`Error::new`] and the `with_*` methods. Boxed, so
/// a `Result` carrying one stays small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(Box<Fields<ErrorCode>>);

/// What an error or a warning says, in the error contract's shape, its code an error's
/// ([`ErrorCode`]) or a warning's ([`WarningCode`]). A warning is never worded from a worker,
/// so its `worker` and `advice` stay empty.
#[derive(Debug, Clone, Serialize)]
struct Fields<C> {
    code: C,
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
    /// ([`Error::with_advice`]): written out as its word ([`Advice::word`], the contract's
    /// `advice`, null for none), and dropped by any other suggestion given after.
    #[serde(serialize_with = "advice_word")]
    advice: Option<Box<Advice>>,
}

/// `advice` as the error contract writes it: its word, or null.
fn advice_word<S: serde::Serializer>(
    advice: &Option<Box<Advice>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match advice.as_deref().and_then(Advice::word) {
        Some(word) => serializer.serialize_str(word),
        None => serializer.serialize_none(),
    }
}

impl<C: PartialEq> PartialEq for Fields<C> {
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
            advice,
        } = self;
        let word = |advice: &Option<Box<Advice>>| advice.as_deref().and_then(Advice::word);
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
            word(advice),
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
            word(&other.advice),
        )
    }
}

impl<C: Eq> Eq for Fields<C> {}

impl<C> Fields<C> {
    /// `code` and `message`, and nothing else said yet.
    fn new(code: C, message: String) -> Self {
        Fields {
            code,
            message,
            feature: None,
            elements: Vec::new(),
            elements_total: 0,
            parameter: None,
            cause: None,
            suggestion: None,
            candidates: Vec::new(),
            worker: None,
            advice: None,
        }
    }

    /// `elements_total` never fewer than `elements` names.
    fn count_elements(&mut self) {
        let named = self.elements.len() as u64;
        self.elements_total = self.elements_total.max(named);
    }
}

impl<C: Copy> Fields<C> {
    /// What it says, every field of it the error contract writes, its code as `word` writes
    /// it.
    fn said(&self, word: impl Fn(C) -> &'static str) -> Said<'_> {
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
            // What it was worded from, never written out.
            worker: _,
            advice,
        } = self;
        Said {
            code: word(*code),
            message,
            feature: feature.as_deref(),
            elements,
            elements_total: *elements_total,
            parameter: parameter.as_deref(),
            cause: cause.as_deref(),
            suggestion: suggestion.as_deref(),
            candidates,
            advice: advice.as_deref().and_then(Advice::word),
        }
    }
}

/// What an error or a warning says, every field the error contract writes
/// (`tests/contract/error-full.json`), borrowed: built from the error's fields taken whole,
/// and taken whole by each writer of the contract (the command API's, `check --json`'s), so
/// a field added to an error is a field each of them must write, or it doesn't compile.
#[derive(Debug, Clone, Copy)]
pub struct Said<'a> {
    /// Its code, as the contract writes it (an error's or a warning's).
    pub code: &'static str,
    pub message: &'a str,
    pub feature: Option<&'a str>,
    pub elements: &'a [String],
    /// How many elements are at fault, never fewer than `elements` names.
    pub elements_total: u64,
    pub parameter: Option<&'a str>,
    pub cause: Option<&'a str>,
    pub suggestion: Option<&'a str>,
    pub candidates: &'a [String],
    /// What the suggestion advises, as data, for a worker's fault: `change_the_values`,
    /// `simplify_or_wait`, `try_again`, or `try_once_more`; none for any other error, a
    /// fault whose advice is its words alone, and every warning.
    pub advice: Option<&'static str>,
}

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
        Self(Box::new(Fields::new(code, message.into())))
    }

    /// Its message said in other words, all else kept: a refusal a caller passes on as its
    /// own (a sketch edit refused by its solve, "after the split, the sketch's constraints
    /// conflict: ...").
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.0.message = message.into();
        self
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
        self.0.count_elements();
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
        self.0.count_elements();
        self
    }

    /// How many elements are at fault, where `elements` names only the first of them: never
    /// fewer than it names.
    #[must_use]
    pub fn with_elements_total(mut self, total: u64) -> Self {
        self.0.elements_total = total;
        self.0.count_elements();
        self
    }

    /// The same elements named anew, in place of how they were named (the kernel's number
    /// for one, named by its element map): as many, so the total stands.
    #[must_use]
    pub(crate) fn with_elements_as<I, S>(mut self, elements: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.elements = elements.into_iter().map(Into::into).collect();
        self.0.count_elements();
        self
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

    /// The suggestion, worded again from this error's own advice (a session's wording of a
    /// kept fault, the CLI's with running its command again: `command::fault_suggestion`):
    /// unlike [`with_suggestion`](Self::with_suggestion), the advice is kept, since the words
    /// still say it.
    #[must_use]
    pub fn reworded(mut self, suggestion: impl Into<String>) -> Self {
        self.0.suggestion = Some(suggestion.into());
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

    /// Whether the worker's advice says the request's values crashed it, and the same request
    /// would crash it again: read from the advice as data, never from the suggestion's words.
    /// Meaningful for a kernel fault, as the model read meets it or a session words it
    /// ([`kept_fault`](crate::command)); false for any error without that advice. A client
    /// that adds its own "try again" leaves it out then: the CLI's "run it again".
    pub fn would_crash_again(&self) -> bool {
        matches!(self.advice(), Some(Advice::ChangeTheValues { .. }))
    }

    /// What it says, every field the error contract writes.
    pub fn said(&self) -> Said<'_> {
        self.0.said(ErrorCode::as_str)
    }

    /// Logs the error at info, `what` before its message (`the step failed`), with its
    /// code, the request's field at fault, the elements it names by their names (those it
    /// lists, as a list of strings, `["a", "b"]`, since a name holds spaces, and how many in
    /// all), and its cause: in the span it's logged in, so a session's logs say what failed
    /// by the names the document uses, never by a worker's own numbering.
    pub(crate) fn log(&self, what: &str) {
        let elements = (!self.0.elements.is_empty()).then_some(&self.0.elements);
        let total = (self.0.elements_total > 0).then_some(self.0.elements_total);
        tracing::info!(
            code = self.0.code.as_str(),
            parameter = self.parameter(),
            elements = elements.map(tracing::field::debug),
            elements_total = total,
            cause = self.cause(),
            "{what}: {}",
            self.0.message
        );
    }
}

/// What [`Error`]'s and [`Warning`]'s `Display` write (as [`Error`]'s says).
impl<C: fmt::Display> fmt::Display for Fields<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        let mut about = Vec::new();
        if let Some(feature) = &self.feature {
            about.push(format!("feature {feature}"));
        }
        let unnamed = self
            .elements_total
            .saturating_sub(self.elements.len() as u64);
        match (self.elements.as_slice(), unnamed) {
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
        if let Some(parameter) = &self.parameter {
            about.push(format!("parameter {parameter}"));
        }
        if !about.is_empty() {
            write!(f, " ({})", about.join(", "))?;
        }
        // Text that already ends a sentence doesn't get a second period.
        let sentence = |text: &str| text.strip_suffix('.').unwrap_or(text).to_owned();
        if let Some(cause) = &self.cause {
            write!(f, ": {}", sentence(cause))?;
        }
        if let Some(suggestion) = &self.suggestion {
            write!(f, ". Suggestion: {}", sentence(suggestion))?;
        }
        if !self.candidates.is_empty() {
            write!(f, ". Candidates: {}", self.candidates.join(", "))?;
        }
        if self.suggestion.is_some() || !self.candidates.is_empty() {
            f.write_str(".")?;
        }
        Ok(())
    }
}

/// `code: message (feature F, elements E1, E2, parameter P): cause. Suggestion: S.
/// Candidates: A, B.`, leaving out whatever is absent (`element E` for one; `elements E1, E2,
/// and 3 more` for five, two named).
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl std::error::Error for Error {}

/// Something `check` finds that fails nothing, in the error contract's shape, so an agent
/// reads one shape (docs/design/m4.1-command-api.md, *Warnings*, decision 17): a code from a
/// list of its own ([`WarningCode`]), a message, and, where they apply, the feature, the
/// elements it's about (the first of them, and how many in all), the parameter, the cause,
/// and what to do. Build one with [`Warning::new`] and the `with_*` methods; `Display` and
/// the JSON form are an error's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning(Box<Fields<WarningCode>>);

impl Warning {
    /// A warning with a code and a one-line message saying what was found.
    pub fn new(code: WarningCode, message: impl Into<String>) -> Self {
        Self(Box::new(Fields::new(code, message.into())))
    }

    /// The feature the warning is about, by ID.
    #[must_use]
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.0.feature = Some(feature.into());
        self
    }

    /// Sub-elements the warning is about, after any named already.
    #[must_use]
    pub fn with_elements<I, S>(mut self, elements: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.0.elements.extend(elements.into_iter().map(Into::into));
        self.0.count_elements();
        self
    }

    /// How many elements it's about, where `elements` names only the first of them: never
    /// fewer than it names.
    #[must_use]
    pub fn with_elements_total(mut self, total: u64) -> Self {
        self.0.elements_total = total;
        self.0.count_elements();
        self
    }

    /// The field it's about, as a dotted name.
    #[must_use]
    pub fn with_parameter(mut self, parameter: impl Into<String>) -> Self {
        self.0.parameter = Some(parameter.into());
        self
    }

    /// What lies under it.
    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.0.cause = Some(cause.into());
        self
    }

    /// What the user or agent could do about it.
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.0.suggestion = Some(suggestion.into());
        self
    }

    pub fn code(&self) -> WarningCode {
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

    /// How many elements it's about: as many as [`Warning::elements`] names, or more when
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

    /// What it says, every field the error contract writes (its candidates none: nothing's
    /// broken for it to have meant).
    pub fn said(&self) -> Said<'_> {
        self.0.said(WarningCode::as_str)
    }
}

/// As an error is shown ([`Error`]'s `Display`): `code: message (feature F, elements E1,
/// E2): cause. Suggestion: S.`
impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Warning {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
