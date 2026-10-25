//! The command API's JSON (docs/design/m4.1-command-api.md, sections 2 and 8): every
//! response in one envelope ([`response`]), every result written field for field in schema
//! order ([`ToWire`]), every request read strictly by Silico's own reader ([`Fields`]), and
//! each command's request and result schemas ([`snapshot`]), which the protected snapshots in
//! `tests/contract/commands/` record. Numbers are written as their own text (a `Length`,
//! `Angle`, or `Measure` writes itself) and read from their digits, never through a float.
//! Results are written as the document is, by its canonical writer (M2.1's D7), or on one
//! line for a transport that takes one message a line ([`Layout::OneLine`]).
//!
//! Each command's wire types live in a module of their own here, beside it: its request,
//! read with [`Fields`], its result's [`ToWire`], and the mirror structs its snapshot is
//! generated from.

use std::borrow::Cow;

use schemars::JsonSchema;

use super::{ModelChoice, ModelRead, Problems, Response, Revision, SessionId, SessionInfo};
use crate::document::FeatureId;
use crate::document::json::{Member, Node, Value};
use crate::document::write::{Out, text};
use crate::{Error, ErrorCode};

/// A JSON value to write, built by a [`ToWire`]: opaque, so only this module's builders make
/// one, and every value is written as the canonical writer writes it.
pub struct Json<'a>(Out<'a>);

impl<'a> Json<'a> {
    pub(crate) fn null() -> Self {
        Json(Out::Raw("null".to_owned()))
    }

    pub(crate) fn bool(b: bool) -> Self {
        Json(Out::Raw(if b { "true" } else { "false" }.to_owned()))
    }

    /// A whole number.
    pub(crate) fn whole(n: u64) -> Self {
        Json(Out::Raw(n.to_string()))
    }

    pub(crate) fn str(text: &'a str) -> Self {
        Json(Out::Str(text))
    }

    pub(crate) fn string(text: String) -> Self {
        Json(Out::Owned(text))
    }

    pub(crate) fn array(items: impl IntoIterator<Item = Json<'a>>) -> Self {
        Json(Out::Array(items.into_iter().map(|j| j.0).collect()))
    }

    /// An object, its members in the order given: schema order, never sorted.
    pub(crate) fn object(members: impl IntoIterator<Item = (&'static str, Json<'a>)>) -> Self {
        Json(Out::Object(
            members.into_iter().map(|(key, j)| (key, j.0)).collect(),
        ))
    }

    /// `value`'s JSON, or `null` for none.
    pub(crate) fn or_null<T>(value: Option<T>, f: impl FnOnce(T) -> Json<'a>) -> Self {
        value.map_or_else(Json::null, f)
    }
}

/// How a value is written: in canonical form, an object or array on one line when it holds
/// no object, otherwise a member to a line (M2.1's D7, with no trailing newline: a response
/// is a value, not a file); or all on one line, for a transport that takes one message a line
/// (MCP's stdio). Either way, the same members in the same order with the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Canonical,
    OneLine,
}

/// `value` as text, in `layout`.
pub fn write(value: &Json<'_>, layout: Layout) -> String {
    text(&value.0, layout == Layout::OneLine)
}

/// A command's result, or a part of one, as the API writes it: every field always present,
/// `null` where it doesn't apply, its keys in schema order (section 2).
pub trait ToWire {
    fn to_wire(&self) -> Json<'_>;
}

impl ToWire for Revision {
    /// A bare whole number.
    fn to_wire(&self) -> Json<'_> {
        Json::whole(self.number())
    }
}

impl ToWire for SessionId {
    /// `s1`, `s2`, ...
    fn to_wire(&self) -> Json<'_> {
        Json::string(self.to_string())
    }
}

impl ToWire for ModelChoice {
    fn to_wire(&self) -> Json<'_> {
        Json::str(match self {
            ModelChoice::Current => "current",
            ModelChoice::Published => "published",
        })
    }
}

impl ToWire for SessionInfo {
    /// Its ID, its path as opened, its revision, and whether a transaction is open and its
    /// file was read again. A path that isn't UTF-8 would be written lossily: unreachable
    /// once `open` refuses non-UTF-8 paths (M4.2's PR 5).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("id", self.id.to_wire()),
            (
                "path",
                Json::string(self.path.to_string_lossy().into_owned()),
            ),
            ("revision", self.revision.to_wire()),
            ("transaction", Json::bool(self.transaction)),
            ("reloaded", Json::bool(self.reloaded)),
        ])
    }
}

impl ToWire for ModelRead {
    /// Its revision, whether it was the current or the published, and the kernel fault its
    /// rebuild met, none until faults are kept as a revision's outcome (decision 32).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            ("of", self.of.to_wire()),
            ("fault", Json::null()),
        ])
    }
}

impl ToWire for Error {
    /// The error contract's shape (`tests/contract/error-full.json`), field for field.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("code", Json::str(self.code().as_str())),
            ("message", Json::str(self.message())),
            ("feature", Json::or_null(self.feature(), Json::str)),
            (
                "elements",
                Json::array(self.elements().iter().map(|e| Json::str(e))),
            ),
            ("elements_total", Json::whole(self.elements_total())),
            ("parameter", Json::or_null(self.parameter(), Json::str)),
            ("cause", Json::or_null(self.cause(), Json::str)),
            ("suggestion", Json::or_null(self.suggestion(), Json::str)),
            (
                "candidates",
                Json::array(self.candidates().iter().map(|c| Json::str(c))),
            ),
        ])
    }
}

/// A list of problems as a result or the envelope writes them: the first
/// [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) errors, in the error contract's
/// shape, and how many there are, listed or not (`problems` and `problems_total` in a
/// result, `errors` and `total` in a refusal).
pub fn problems(problems: &Problems) -> (Json<'_>, Json<'_>) {
    (
        Json::array(
            problems
                .errors()
                .iter()
                .take(crate::document::PROBLEMS_LISTED)
                .map(ToWire::to_wire),
        ),
        Json::whole(problems.total()),
    )
}

/// A command's response, in its envelope (section 2): `ok`, the session as the command left
/// it, the model it read, and its result; or, refused, every reason, the first
/// `PROBLEMS_LISTED`, and how many there are.
pub fn response<T: ToWire>(response: &Response<T>) -> Json<'_> {
    let session = Json::or_null(response.session.as_ref(), ToWire::to_wire);
    let model = Json::or_null(response.model.as_ref(), ToWire::to_wire);
    match &response.result {
        Ok(result) => Json::object([
            ("ok", Json::bool(true)),
            ("session", session),
            ("model", model),
            ("result", result.to_wire()),
        ]),
        Err(refused) => {
            let (errors, total) = problems(refused);
            Json::object([
                ("ok", Json::bool(false)),
                ("session", session),
                ("model", model),
                ("errors", errors),
                ("total", total),
            ])
        }
    }
}

/// A request's fields, read strictly (section 2's *Requests are strict*): each field taken by
/// name and of its type, every field the shape gives required, `null` only where the shape
/// allows it, and, at the end ([`Fields::done`]), any key not taken refused. Every reason is
/// kept, in the order found, each an `invalid_input` error naming its field by its dotted
/// path (decision 15: here, a member of a list by its place, from 0).
pub struct Fields<'a> {
    path: String,
    members: &'a [Member],
    taken: Vec<bool>,
    /// The first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) refusals, in the
    /// order found; the rest are only counted, so a request of a million bad values holds
    /// a hundred errors, not a million.
    refused: Vec<Error>,
    unlisted: u64,
}

impl<'a> Fields<'a> {
    /// The fields of `node`, which must be an object; `path` is where it lies in the request
    /// (empty at its top).
    pub fn of(node: &'a Node, path: &str) -> Result<Self, Problems> {
        match &node.value {
            Value::Object(members) => Ok(Fields {
                path: path.to_owned(),
                members,
                taken: vec![false; members.len()],
                refused: Vec::new(),
                unlisted: 0,
            }),
            other => Err(invalid(
                path,
                format!("{} must be an object, got {}", shown(path), other.kind()),
            )
            .into()),
        }
    }

    /// Keeps `error`, if fewer than [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED)
    /// are kept, and counts it either way.
    fn refuse(&mut self, error: Error) {
        if self.refused.len() < crate::document::PROBLEMS_LISTED {
            self.refused.push(error);
        } else {
            self.unlisted += 1;
        }
    }

    /// Keeps `listed`, refusals found elsewhere, and counts `unlisted` more.
    fn refuse_all(&mut self, listed: impl IntoIterator<Item = Error>, unlisted: u64) {
        for error in listed {
            self.refuse(error);
        }
        self.unlisted += unlisted;
    }

    /// The dotted path of `key` here.
    fn at(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    /// The value of `key`, marked taken; refused if it's missing.
    fn take(&mut self, key: &str) -> Option<&'a Node> {
        let found = self.members.iter().position(|m| m.key == key);
        match found {
            Some(i) => {
                if let Some(taken) = self.taken.get_mut(i) {
                    *taken = true;
                }
                self.members.get(i).map(|m| &m.value)
            }
            None => {
                let at = self.at(key);
                self.refuse(invalid(&at, format!("{at} is missing")).with_suggestion(
                    "give every field the request's shape has; null where it allows none",
                ));
                None
            }
        }
    }

    /// The value of `key`, or `Some(None)` if it's `null`.
    fn take_nullable(&mut self, key: &str) -> Option<Option<&'a Node>> {
        let node = self.take(key)?;
        Some(match node.value {
            Value::Null => None,
            _ => Some(node),
        })
    }

    /// A refusal of `key`'s value: `what` it must be, and what it was.
    fn wrong(&mut self, key: &str, what: &str, node: &Node) {
        let at = self.at(key);
        self.refuse(invalid(
            &at,
            format!("{at} must be {what}, got {}", node.value.kind()),
        ));
    }

    /// A string.
    pub fn string(&mut self, key: &str) -> Option<&'a str> {
        let node = self.take(key)?;
        match &node.value {
            Value::String(text) => Some(text),
            _ => {
                self.wrong(key, "a string", node);
                None
            }
        }
    }

    /// A string, or `null`.
    pub fn nullable_string(&mut self, key: &str) -> Option<Option<&'a str>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => match &node.value {
                Value::String(text) => Some(Some(text)),
                _ => {
                    self.wrong(key, "a string or null", node);
                    None
                }
            },
        }
    }

    /// `true` or `false`.
    pub fn bool(&mut self, key: &str) -> Option<bool> {
        let node = self.take(key)?;
        match node.value {
            Value::Bool(b) => Some(b),
            _ => {
                self.wrong(key, "true or false", node);
                None
            }
        }
    }

    /// A whole number from 0 to `u32::MAX`, written without a fraction or exponent.
    pub fn u32(&mut self, key: &str) -> Option<u32> {
        let node = self.take(key)?;
        let whole = match &node.value {
            Value::Number(text) => whole(text),
            _ => None,
        };
        if whole.is_none() {
            let at = self.at(key);
            self.refuse(invalid(
                &at,
                format!(
                    "{at} must be a whole number from 0 to {}, got {}",
                    u32::MAX,
                    written(node)
                ),
            ));
        }
        whole
    }

    /// The session a request names: its ID, `s1`, `s2`, ...
    pub fn session(&mut self) -> Option<SessionId> {
        let node = self.take("session")?;
        let id = match &node.value {
            Value::String(text) => SessionId::parse(text),
            _ => None,
        };
        if id.is_none() {
            let at = self.at("session");
            self.refuse(
                invalid(
                    &at,
                    format!(
                        "{at} must be a session's ID, s1, s2, ..., got {}",
                        written(node)
                    ),
                )
                .with_suggestion("give the ID open returned"),
            );
        }
        id
    }

    /// Which model a read takes (decision 33): `current` or `published`.
    pub fn model(&mut self) -> Option<ModelChoice> {
        let node = self.take("model")?;
        let choice = match &node.value {
            Value::String(text) if text == "current" => Some(ModelChoice::Current),
            Value::String(text) if text == "published" => Some(ModelChoice::Published),
            _ => None,
        };
        if choice.is_none() {
            let at = self.at("model");
            self.refuse(
                invalid(
                    &at,
                    format!("{at} must be current or published, got {}", written(node)),
                )
                .with_candidates(["current", "published"]),
            );
        }
        choice
    }

    /// A feature's ID, or `null`.
    pub fn nullable_feature(&mut self, key: &str) -> Option<Option<FeatureId>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => {
                let id = match &node.value {
                    Value::String(text) => FeatureId::new(text),
                    _ => None,
                };
                if id.is_none() {
                    let at = self.at(key);
                    self.refuse(invalid(
                        &at,
                        format!("{at} must be a feature's ID or null, got {}", written(node)),
                    ));
                }
                id.map(Some)
            }
        }
    }

    /// A list of features' IDs, or `null`; a member at fault named by its place
    /// (`features.0`).
    pub fn nullable_features(&mut self, key: &str) -> Option<Option<Vec<FeatureId>>> {
        let node = match self.take_nullable(key)? {
            None => return Some(None),
            Some(node) => node,
        };
        let Value::Array(items) = &node.value else {
            self.wrong(key, "a list of features' IDs or null", node);
            return None;
        };
        let at = self.at(key);
        let mut ids = Vec::with_capacity(items.len());
        let mut fine = true;
        for (i, item) in items.iter().enumerate() {
            match &item.value {
                Value::String(text) if let Some(id) = FeatureId::new(text) => ids.push(id),
                _ => {
                    fine = false;
                    self.refuse(invalid(
                        &format!("{at}.{i}"),
                        format!("{at}.{i} must be a feature's ID, got {}", written(item)),
                    ));
                }
            }
        }
        fine.then_some(Some(ids))
    }

    /// An object's fields, or `null`, at `key`.
    pub fn nullable_object(&mut self, key: &str) -> Option<Option<Fields<'a>>> {
        let at = self.at(key);
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => match Fields::of(node, &at) {
                Ok(fields) => Some(Some(fields)),
                Err(refused) => {
                    let unlisted = refused
                        .total()
                        .saturating_sub(refused.errors().len() as u64);
                    self.refuse_all(refused.errors().iter().cloned(), unlisted);
                    None
                }
            },
        }
    }

    /// Keeps `inner`'s refusals, an object read from one of these fields, with these.
    pub fn absorb(&mut self, inner: Fields<'a>) {
        let (listed, unlisted) = inner.finish();
        self.refuse_all(listed, unlisted);
    }

    /// Every key not taken, refused, and every refusal so far, the first
    /// [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) listed and all counted; or `Ok`
    /// if there are none.
    pub fn done(self) -> Result<(), Problems> {
        let (refused, unlisted) = self.finish();
        if refused.is_empty() {
            return Ok(());
        }
        Err(Problems::with_unlisted(refused, unlisted))
    }

    /// The refusals listed and how many more there are, every key not taken refused. A
    /// key is named as the document's reader names one (`field_name`): as written if it's a
    /// plain name, otherwise quoted, escaped, and cut short, so no key makes its refusal
    /// unsafe to print, long, or a path that names another field (`a.b`).
    fn finish(mut self) -> (Vec<Error>, u64) {
        let (members, taken) = (self.members, std::mem::take(&mut self.taken));
        for (member, taken) in members.iter().zip(taken) {
            if !taken {
                let at = self.at(&crate::document::field_name(&member.key));
                self.refuse(
                    invalid(&at, format!("{at} isn't a field of this request")).with_suggestion(
                        "leave it out: a request has only the fields its shape gives",
                    ),
                );
            }
        }
        (self.refused, self.unlisted)
    }
}

/// An `invalid_input` error naming `path`.
fn invalid(path: &str, message: String) -> Error {
    let error = Error::new(ErrorCode::InvalidInput, message);
    if path.is_empty() {
        error
    } else {
        error.with_parameter(path)
    }
}

/// The request, or its field `path`, for a message.
fn shown(path: &str) -> Cow<'_, str> {
    if path.is_empty() {
        Cow::Borrowed("the request")
    } else {
        Cow::Borrowed(path)
    }
}

/// A number's text as a whole `u32`: digits only, no leading zeros but `0` itself.
fn whole(text: &str) -> Option<u32> {
    let digits = text.bytes().all(|b| b.is_ascii_digit());
    let leading = text.len() > 1 && text.starts_with('0');
    if text.is_empty() || !digits || leading {
        return None;
    }
    text.parse().ok()
}

/// What a value was, for a refusal: a number or string as written (a string quoted and
/// escaped, either cut short if it's long), else its kind.
fn written(node: &Node) -> String {
    match &node.value {
        Value::Number(text) => crate::document::shown_number(text),
        Value::String(text) => crate::document::json::shown(text),
        other => other.kind().to_owned(),
    }
}

/// The error contract's shape, for the snapshots: the envelope's `errors` and every
/// result's `problems` reference this one schema.
#[derive(JsonSchema)]
pub struct ErrorSchema {
    pub code: String,
    pub message: String,
    pub feature: Option<String>,
    pub elements: Vec<String>,
    pub elements_total: u64,
    pub parameter: Option<String>,
    pub cause: Option<String>,
    pub suggestion: Option<String>,
    pub candidates: Vec<String>,
}

/// A session as a response gives it, for the envelope's snapshot.
#[derive(JsonSchema)]
pub struct SessionSchema {
    /// `s1`, `s2`, ...
    pub id: String,
    pub path: String,
    pub revision: u64,
    pub transaction: bool,
    pub reloaded: bool,
}

/// Which model was read, for the envelope's snapshot.
#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum ModelOfSchema {
    Current,
    Published,
}

/// The model a command read, for the envelope's snapshot.
#[derive(JsonSchema)]
pub struct ModelSchema {
    pub revision: u64,
    pub of: ModelOfSchema,
    /// The kernel fault its rebuild met, once (decision 32): always null until then.
    pub fault: Option<ErrorSchema>,
}

/// A command that ran: its result, which each command's snapshot gives.
#[derive(JsonSchema)]
pub struct RanSchema {
    #[schemars(extend("const" = true))]
    pub ok: bool,
    pub session: Option<SessionSchema>,
    pub model: Option<ModelSchema>,
    pub result: serde_json::Value,
}

/// A command refused: every reason, the first hundred, and how many there are.
#[derive(JsonSchema)]
pub struct RefusedSchema {
    #[schemars(extend("const" = false))]
    pub ok: bool,
    pub session: Option<SessionSchema>,
    pub model: Option<ModelSchema>,
    pub errors: Vec<ErrorSchema>,
    pub total: u64,
}

/// Every response: one that ran, or one refused.
#[derive(JsonSchema)]
#[schemars(untagged)]
pub enum EnvelopeSchema {
    Ran(RanSchema),
    Refused(RefusedSchema),
}

/// A command's snapshot: its request's schema and its result's, under one root, so the
/// two share their definitions.
#[derive(JsonSchema)]
pub struct CommandSchema<Request: JsonSchema, Result: JsonSchema> {
    pub request: Request,
    pub result: Result,
}

/// The JSON Schema snapshot of `of` (section 8): `envelope`, the response's; or a command's,
/// `{"request": ..., "result": ...}`. None for a name with no snapshot. Written as the
/// document's schema is, its keys sorted, pretty, with a trailing newline.
pub fn snapshot(of: &str) -> Option<Result<String, Error>> {
    match of {
        "envelope" => Some(crate::document::schema_of::<EnvelopeSchema>(
            "the envelope",
            crate::document::Presence::Always,
        )),
        _ => None,
    }
}

/// The names [`snapshot`] knows, in order.
pub const SNAPSHOTS: &[&str] = &["envelope"];

#[cfg(test)]
mod adversarial;
#[cfg(test)]
mod tests;
