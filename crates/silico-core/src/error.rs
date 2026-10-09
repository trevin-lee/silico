//! Silico's structured error (ARCHITECTURE.md, invariant 9). Every failure that reaches a
//! user or an agent is one of these: a stable code to match on, a message, and where they
//! apply, the feature, the sub-element, the underlying cause, a suggestion, and candidate
//! fixes.
//!
//! The JSON form is the contract with agents (tests/contract/): every field is always
//! present, `null` when absent and `[]` for no candidates, so there's one shape to parse.

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
    element: Option<String>,
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
            element: None,
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

    /// The sub-element (face, edge, vertex, sketch entity) the error is about.
    #[must_use]
    pub fn with_element(mut self, element: impl Into<String>) -> Self {
        self.0.element = Some(element.into());
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

    pub fn element(&self) -> Option<&str> {
        self.0.element.as_deref()
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

/// `code: message (feature F, element E): cause. Suggestion: S. Candidates: A, B.`,
/// leaving out whatever is absent.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.code, self.0.message)?;
        match (&self.0.feature, &self.0.element) {
            (Some(feature), Some(element)) => write!(f, " (feature {feature}, element {element})")?,
            (Some(feature), None) => write!(f, " (feature {feature})")?,
            (None, Some(element)) => write!(f, " (element {element})")?,
            (None, None) => {}
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
