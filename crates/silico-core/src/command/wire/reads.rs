//! `text`, `outline`, and `check`'s wire types (docs/design/m4.1-command-api.md, section 7):
//! their requests, read strictly ([`TextRequest`], [`CheckRequest`], and the
//! [`SessionRequest`](super::transaction::SessionRequest) `outline` takes); their results,
//! written field for field ([`ToWire`]: [`TextAnswer`], [`OutlineAnswer`], and
//! [`CheckAnswer`], what the session's commands give); and the
//! mirrors their snapshots, `commands/text.json`, `outline.json`, and `check.json`, are
//! generated from.
//!
//! `check`'s `fresh` (M4.3: the current revision rebuilt with every step built anew,
//! [`Sessions::check_fresh`](crate::command::Sessions::check_fresh)) is taken only with the
//! current model: a published read never rebuilds.

use schemars::JsonSchema;

use super::elements::ModelChoiceSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{CheckAnswer, ModelChoice, OutlineAnswer, Problems, SessionId, TextAnswer};
use crate::document::json::Node;
use crate::document::{FeatureId, PROBLEMS_LISTED};
use crate::{Error, ErrorCode};

/// A `text` request: the session, and the feature to read alone (`None` for the whole
/// document).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRequest {
    pub session: SessionId,
    pub feature: Option<FeatureId>,
}

impl TextRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let feature = f.nullable_feature("feature");
            Some(TextRequest {
                session: session?,
                feature: feature?,
            })
        })
    }
}

impl ToWire for TextRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            (
                "feature",
                Json::or_null(self.feature.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

/// A `check` request: the session, the model it reads, and whether the current one is
/// rebuilt with every step built anew, as
/// [`Sessions::check`](crate::command::Sessions::check) and
/// [`Sessions::check_fresh`](crate::command::Sessions::check_fresh) take them. `fresh` is
/// only ever true with [`ModelChoice::Current`]: the reader refuses it with the published
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub fresh: bool,
}

impl CheckRequest {
    /// The request `node` holds, or every reason its shape is refused: `fresh` true with the
    /// published model among them (a published read never rebuilds), naming `fresh`.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let fresh = f.bool("fresh");
            if model == Some(ModelChoice::Published) && fresh == Some(true) {
                f.refuse(fresh_published());
                return None;
            }
            Some(CheckRequest {
                session: session?,
                model: model?,
                fresh: fresh?,
            })
        })
    }
}

/// The refusal of `fresh` with the published model, which is read as it is, never rebuilt
/// (decision 33): as a request's fields that must agree are refused (a camera's
/// projection's), naming the field to change.
fn fresh_published() -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        "fresh must be false for a read of the published model",
    )
    .with_parameter("fresh")
    .with_cause(
        "a read of the published model answers from the newest model as it is, rebuilding nothing",
    )
    .with_suggestion(
        "give model current with fresh true to rebuild the current revision anew, or fresh \
         false to read the published model",
    )
}

impl ToWire for CheckRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("fresh", Json::bool(self.fresh)),
        ])
    }
}

impl ToWire for TextAnswer {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("text", Json::str(&self.text))])
    }
}

impl ToWire for OutlineAnswer {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("outline", Json::str(&self.outline))])
    }
}

impl ToWire for CheckAnswer {
    /// The problems, the first 100 and how many, and the warnings, likewise.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        let (warnings, warnings_total) = super::problems(&self.warnings);
        Json::object([
            ("problems", problems),
            ("problems_total", problems_total),
            ("warnings", warnings),
            ("warnings_total", warnings_total),
        ])
    }
}

/// `text`'s request: the document's text as the session holds it, or one feature's.
#[derive(JsonSchema)]
#[schemars(rename = "TextRequest", transform = wire_shape)]
pub struct TextRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The feature whose text to give alone, by ID, or null for the whole document. For a
    /// document too large to give in one response, read it a feature at a time.
    pub feature: Option<FeatureId>,
}

/// `text`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "TextResult", transform = wire_shape)]
pub struct TextResultSchema {
    /// The whole document's text: the file's bytes until an edit, then the edited document
    /// in canonical form (inside a transaction, as yet uncommitted). Or one feature's, as
    /// the canonical form writes it.
    pub text: String,
}

/// `outline`'s request: the document's outline, one line a feature, as `silico show`
/// prints it.
#[derive(JsonSchema)]
#[schemars(rename = "OutlineRequest", transform = wire_shape)]
pub struct OutlineRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// `outline`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "OutlineResult", transform = wire_shape)]
pub struct OutlineResultSchema {
    /// The outline: each feature's ID, name, type, and values, with its stored fingerprint,
    /// a line each, its note on the line after; then the bodies.
    pub outline: String,
}

/// `check`'s request: the document's problems and warnings, from the model named.
#[derive(JsonSchema)]
#[schemars(rename = "CheckRequest", transform = wire_shape)]
pub struct CheckRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// Whether to rebuild the current revision with every step built anew, reading nothing
    /// kept, in memory or in the rebuild cache (still keeping what it builds, so later
    /// rebuilds hit it), as a forced rebuild does: it tries again a kernel fault the
    /// revision keeps. Usually false. True only with model current: a read of the published
    /// model never rebuilds.
    pub fresh: bool,
}

/// `check`'s result. A kernel fault isn't the document's: it's in the response's
/// `model.fault`, never here.
#[derive(JsonSchema)]
#[schemars(rename = "CheckResult", transform = wire_shape)]
pub struct CheckResultSchema {
    /// The document's problems, the first 100: the gate's, if it refused the document;
    /// otherwise each failed feature's, and each drift of a feature no edit has reached
    /// since the last save.
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub problems: Vec<ErrorSchema>,
    /// How many problems there are, listed or not.
    pub problems_total: u64,
    /// The document's warnings, the first 100, in the error contract's shape: they never
    /// fail anything.
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub warnings: Vec<ErrorSchema>,
    /// How many warnings there are, listed or not.
    pub warnings_total: u64,
}
