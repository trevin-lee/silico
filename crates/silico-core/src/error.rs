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

use std::fmt;

use serde::Serialize;

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
    /// A worker process crashed while handling a request.
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
}

impl ErrorCode {
    /// Whether the code is the kernel's own trouble, which says nothing about what was
    /// asked of it: a worker that crashed or hung, a reply that couldn't be read, a
    /// shortage, or a bug in Silico. A command reports one as its own failure (it couldn't
    /// run), not as a problem of the document, and the oracle as a case it couldn't
    /// finish. Every code answers here, so a new code has to say.
    pub fn is_kernel_fault(self) -> bool {
        match self {
            ErrorCode::WorkerCrashed
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
            | ErrorCode::SolveFailed => false,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        }))
    }

    /// The feature the error is about, by name.
    #[must_use]
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.0.feature = Some(feature.into());
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
        self
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
