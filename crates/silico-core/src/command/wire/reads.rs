//! `text`, `outline`, and `check`'s wire types (docs/design/m4.1-command-api.md, section 7):
//! their requests, read strictly ([`TextRequest`], [`CheckRequest`], and the
//! [`SessionRequest`](super::transaction::SessionRequest) `outline` takes); their results,
//! written field for field ([`ToWire`]: [`TextAnswer`], [`OutlineAnswer`], and
//! [`CheckAnswer`], each what its session command gives, under its result's name); and the
//! mirrors their snapshots, `commands/text.json`, `outline.json`, and `check.json`, are
//! generated from.
//!
//! `check`'s `fresh` (a rebuild that reads no cache) comes with M4.3: until then the request
//! has it, as the design's does, and takes only `false`.

use schemars::JsonSchema;

use super::elements::ModelChoiceSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, problems, wire_shape};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{FeatureId, PROBLEMS_LISTED};

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

/// A `check` request: the session, and the model it reads, as
/// [`Sessions::check_model`](crate::command::Sessions::check_model) takes them. Its `fresh`
/// is read too, and only `false` taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckRequest {
    pub session: SessionId,
    pub model: ModelChoice,
}

impl CheckRequest {
    /// The request `node` holds, or every reason its shape is refused: `fresh` true among
    /// them, until a fresh check is built.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let fresh = f.only_false(
                "fresh",
                "a check that rebuilds without reading the rebuild cache isn't built yet",
                "give false; to try a kernel fault the revision keeps again, close the session \
                 and open the document again",
            );
            fresh?;
            Some(CheckRequest {
                session: session?,
                model: model?,
            })
        })
    }
}

impl ToWire for CheckRequest {
    /// The request as a client writes it, every field in the shape's order: `fresh` false.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("fresh", Json::bool(false)),
        ])
    }
}

/// What `text` answers: the text the session's document is, or one feature's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAnswer {
    pub text: String,
}

impl ToWire for TextAnswer {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("text", Json::str(&self.text))])
    }
}

/// What `outline` answers: the document's outline, as `silico show` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineAnswer {
    pub outline: String,
}

impl ToWire for OutlineAnswer {
    fn to_wire(&self) -> Json<'_> {
        Json::object([("outline", Json::str(&self.outline))])
    }
}

/// What `check` answers: the document's problems, as the session's `check` gives them, and
/// its warnings, of which there are none yet (the first, M3's two curve ends at one place
/// that aren't one point, comes with its code).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckAnswer {
    pub problems: Problems,
}

impl ToWire for CheckAnswer {
    /// The problems, the first 100 and how many, and the warnings, likewise.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        Json::object([
            ("problems", problems),
            ("problems_total", problems_total),
            ("warnings", Json::array([])),
            ("warnings_total", Json::whole(0)),
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
    /// Only false, for now: a check that rebuilds every step without reading the rebuild
    /// cache isn't built yet, and true is refused.
    #[schemars(extend("const" = false))]
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
