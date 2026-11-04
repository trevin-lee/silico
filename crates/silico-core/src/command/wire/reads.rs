//! `text`, `outline`, `check`, `timeline`, and `diff`'s wire types
//! (docs/design/m4.1-command-api.md, section 7; docs/design/m5.0-app.md, section 7, for
//! `timeline`; Fable's decision of 2026-11-04, section 1, for `diff`): their requests, read
//! strictly ([`TextRequest`], [`CheckRequest`], [`TimelineRequest`], [`DiffRequest`], and the
//! [`SessionRequest`](super::transaction::SessionRequest) `outline` takes); their results,
//! written field for field ([`ToWire`]: [`TextAnswer`], [`OutlineAnswer`], [`CheckAnswer`],
//! [`TimelineAnswer`], and [`DiffAnswer`], what the session's commands give); and the
//! mirrors their snapshots, `commands/text.json`, `outline.json`, `check.json`,
//! `timeline.json`, and `diff.json`, are generated from. A timeline entry's health is written as [`Health::word`] gives it, and its
//! snapshot's words are [`Health::ALL`]'s.
//!
//! `check`'s `fresh` (M4.3: the current revision rebuilt with every step built anew) is
//! taken only with the current model: a published read never rebuilds. The request's
//! [`CheckModel`] is what [`Sessions::check`](crate::command::Sessions::check) takes.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

use super::elements::ModelChoiceSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, WarningSchema, problems, wire_shape, words};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{
    CheckAnswer, CheckModel, DiffAnswer, ModelChoice, OutlineAnswer, Problems, Revision, SessionId,
    TextAnswer, TimelineAnswer,
};
use crate::document::json::Node;
use crate::document::{Change, FeatureId, PROBLEMS_LISTED};
use crate::timeline::{Entry, Health};
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

/// A `check` request: the session, and the model it reads (the wire's `model` and
/// `fresh`), as [`Sessions::check`](crate::command::Sessions::check) takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckRequest {
    pub session: SessionId,
    pub model: CheckModel,
}

impl CheckRequest {
    /// The request `node` holds, or every reason its shape is refused: `fresh` true with the
    /// published model among them (a published read never rebuilds), naming `fresh`.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = match (f.model(), f.bool("fresh")) {
                (Some(ModelChoice::Current), Some(fresh)) => CheckModel::Current { fresh },
                (Some(ModelChoice::Published), Some(false)) => CheckModel::Published,
                (Some(ModelChoice::Published), Some(true)) => {
                    f.refuse(fresh_published());
                    return None;
                }
                (None, _) | (_, None) => return None,
            };
            Some(CheckRequest {
                session: session?,
                model,
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
        let (choice, fresh) = match self.model {
            CheckModel::Current { fresh } => (ModelChoice::Current, fresh),
            CheckModel::Published => (ModelChoice::Published, false),
        };
        Json::object([
            ("session", self.session.to_wire()),
            ("model", Json::str(choice.as_str())),
            ("fresh", Json::bool(fresh)),
        ])
    }
}

/// A `timeline` request: the session, and the model whose marks the features carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineRequest {
    pub session: SessionId,
    pub model: ModelChoice,
}

impl TimelineRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            Some(TimelineRequest {
                session: session?,
                model: model?,
            })
        })
    }
}

impl ToWire for TimelineRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
        ])
    }
}

impl ToWire for Entry {
    /// A feature as the timeline draws it: its ID, type, and name, whether it's suppressed,
    /// the features it names, its health, and how many problems and warnings it has.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("id", self.id.to_wire()),
            ("type", Json::str(self.kind)),
            ("name", Json::str(&self.name)),
            ("suppressed", Json::bool(self.suppressed)),
            ("inputs", Json::ids(&self.inputs)),
            ("health", Json::str(self.health.word())),
            ("problems", Json::whole(self.problems)),
            ("warnings", Json::whole(self.warnings)),
        ])
    }
}

impl ToWire for TimelineAnswer {
    /// The session's revision, the model's, the gate's count of problems, the features (or
    /// `null` for text that doesn't read as a document), and the bodies.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            (
                "model_revision",
                Json::or_null(self.model.as_ref(), ToWire::to_wire),
            ),
            ("problems", Json::whole(self.problems)),
            (
                "features",
                Json::or_null(self.features.as_deref(), |features| {
                    Json::array(features.iter().map(ToWire::to_wire))
                }),
            ),
            ("bodies", Json::ids(&self.bodies)),
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

/// A `diff` request: the session, and the two revisions to compare, each `None` for the
/// session's revision now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffRequest {
    pub session: SessionId,
    pub from: Option<Revision>,
    pub to: Option<Revision>,
}

impl DiffRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let from = f.nullable_revision("from");
            let to = f.nullable_revision("to");
            Some(DiffRequest {
                session: session?,
                from: from?,
                to: to?,
            })
        })
    }
}

impl ToWire for DiffRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("from", Json::or_null(self.from.as_ref(), ToWire::to_wire)),
            ("to", Json::or_null(self.to.as_ref(), ToWire::to_wire)),
        ])
    }
}

impl ToWire for Change {
    /// A change: the feature and its name, the field's path (`null` for its place), and
    /// the values before and after, a field's laid out as the document lays it out.
    fn to_wire(&self) -> Json<'_> {
        let value = |value| match &self.field {
            Some(field) => Json::field(field, value),
            None => Json::plain(value),
        };
        Json::object([
            ("feature", self.feature.to_wire()),
            ("name", Json::str(&self.name)),
            ("field", Json::or_null(self.field.as_deref(), Json::str)),
            ("before", value(&self.before)),
            ("after", value(&self.after)),
        ])
    }
}

impl ToWire for DiffAnswer {
    /// The revisions compared, and each change, in order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("from", self.from.to_wire()),
            ("to", self.to.to_wire()),
            (
                "changes",
                Json::array(self.changes.iter().map(ToWire::to_wire)),
            ),
        ])
    }
}

impl ToWire for CheckAnswer {
    /// The problems, the first 100 and how many, and the warnings, likewise.
    fn to_wire(&self) -> Json<'_> {
        let (problems, problems_total) = problems(&self.problems);
        let (warnings, warnings_total) = super::warnings(&self.warnings);
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
    pub warnings: Vec<WarningSchema>,
    /// How many warnings there are, listed or not.
    pub warnings_total: u64,
}

/// `timeline`'s request: the document's features as the browser and the timeline draw them,
/// each with its health, from the model named.
#[derive(JsonSchema)]
#[schemars(rename = "TimelineRequest", transform = wire_shape)]
pub struct TimelineRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
}

/// `timeline`'s result. The features are the session's document's at its revision; their
/// health is the model's, of `model_revision`, which may be older.
#[derive(JsonSchema)]
#[schemars(rename = "TimelineResult", transform = wire_shape)]
pub struct TimelineResultSchema {
    /// The session's revision, whose document the features are.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// The revision of the model the health comes from, as the response's `model` names
    /// it: older than `revision` where the published model is an older revision's. Null
    /// with no model to read (a document the gate refused, with nothing published).
    #[schemars(range(min = 1))]
    pub model_revision: Option<u64>,
    /// How many problems the gate found in the document (or the reader, in text that
    /// doesn't read as one): 0 for a document the gate took, whose model's problems
    /// `check` gives.
    pub problems: u64,
    /// The features, in timeline order: null if the text doesn't read as a document. While
    /// the gate refuses the document, every health is `unknown`, and an ID used twice is
    /// listed twice, so don't key entries by ID then.
    pub features: Option<Vec<TimelineEntrySchema>>,
    /// The bodies the model has, in timeline order: the solids no later feature consumes.
    /// Empty with no model.
    pub bodies: Vec<FeatureId>,
}

/// A feature as the browser and the timeline draw it.
#[derive(JsonSchema)]
#[schemars(rename = "TimelineEntry", transform = wire_shape)]
pub struct TimelineEntrySchema {
    /// Its ID. Unique, but in a document the gate refuses for an ID used twice.
    pub id: FeatureId,
    /// Its type, as the document writes it (`sketch`, `pad`, `fillet`, ...).
    #[schemars(rename = "type")]
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Whether it's suppressed.
    pub suppressed: bool,
    /// The features it names: its `base` and `tool`, then its `sketch`, then, for a sketch on
    /// a face, the feature its `plane.face` begins with, in that order.
    pub inputs: Vec<FeatureId>,
    pub health: HealthSchema,
    /// How many problems the model has on it, whose text `check` gives.
    pub problems: u64,
    /// How many warnings the model has on it, whose text `check` gives.
    pub warnings: u64,
}

/// `diff`'s request: what changed between two revisions the session holds.
#[derive(JsonSchema)]
#[schemars(rename = "DiffRequest", transform = wire_shape)]
pub struct DiffRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The revision to compare from, or null for the document now. A session holds the
    /// text of its revision now, of the one its last reload replaced (after a response that
    /// says `reloaded`), and, inside a transaction, of the one it began at; any other is
    /// refused, the revisions it holds its candidates.
    #[schemars(range(min = 1))]
    pub from: Option<u64>,
    /// The revision to compare to, or null for the document now, held as `from` is.
    #[schemars(range(min = 1))]
    pub to: Option<u64>,
}

/// `diff`'s result.
#[derive(JsonSchema)]
#[schemars(rename = "DiffResult", transform = wire_shape)]
pub struct DiffResultSchema {
    /// The revision compared from: the request's, or, for null, the session's then.
    #[schemars(range(min = 1))]
    pub from: u64,
    /// The revision compared to, likewise.
    #[schemars(range(min = 1))]
    pub to: u64,
    /// Each change from `from` to `to`, in `to`'s timeline order (a feature's change of
    /// place first, then its fields in the order the document writes them), then each
    /// feature removed, in `from`'s order. Empty if the two documents are the same.
    pub changes: Vec<DiffChangeSchema>,
}

/// One change between two revisions: a feature's place in the timeline, or one of its
/// fields.
#[derive(JsonSchema)]
#[schemars(rename = "DiffChange", transform = wire_shape)]
pub struct DiffChangeSchema {
    /// The feature, by ID.
    pub feature: FeatureId,
    /// Its name: as `to` has it, or, for a feature removed, as `from` had it.
    pub name: String,
    /// The path to the field that changed, as `set_field` names one (a sketch's members by
    /// ID, other lists by place, from 0), down to the first value that's a number, a string,
    /// true or false, or absent on one side: so `set_field` of `after` there makes the
    /// change, and of `before` reverts it, where `set_field` can set that field. Null for a
    /// change of the feature's place: added, removed, or moved (its order among the
    /// features both revisions hold changed: one removed or added moves none of the others).
    pub field: Option<String>,
    pub before: DiffValueSchema,
    pub after: DiffValueSchema,
}

/// A change's value on one side.
pub struct DiffValueSchema;

impl JsonSchema for DiffValueSchema {
    fn schema_name() -> Cow<'static, str> {
        "DiffValue".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "description": "With a field, its value on that side, as the document writes \
                            that field: a number, a string, true or false, a list, an object, \
                            or null where the feature hasn't the field (or isn't there). With \
                            field null, the feature's place in that side's timeline, from 0, \
                            as remove_feature gives a place: null where it isn't there."
        })
    }
}

/// A feature's health, in Fusion's states ([`Health::ALL`], each as [`Health::word`] writes
/// it).
pub struct HealthSchema;

impl JsonSchema for HealthSchema {
    fn schema_name() -> Cow<'static, str> {
        "Health".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A feature's health, as the model gave it: healthy (built, or solved, with no \
             problem or warning), warning (built with warnings), error (not built, or built \
             failing a check: a failure, a broken reference, drift; or blocked by a failure \
             upstream), suppressed, or unknown (not built by this model: added or edited \
             since, or using one that was; stopped by a kernel fault; or no model at all).",
            Health::ALL.map(Health::word),
        )
    }
}
