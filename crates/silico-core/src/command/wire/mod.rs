//! The command API's JSON (docs/design/m4.1-command-api.md, sections 2 and 8): every
//! response in one envelope ([`response`], written by [`response_text`] within the
//! [`MAX_RESPONSE_LEN`] a response may be), every result written field for field in schema
//! order ([`ToWire`], of [`Json`]), every request read strictly by Silico's own reader
//! ([`RequestFields::read`]), and the schemas the protected snapshots in
//! `tests/contract/commands/` record ([`snapshot`]). Numbers are written as the document
//! writes them and read from their digits, never through a float (section 2's *Numbers*:
//! [`Json::length`], [`Json::angle`], [`Json::measure`], [`Json::coordinate`],
//! [`Json::direction`], and [`Json::point`]; [`RequestFields::length`],
//! [`RequestFields::angle`], [`RequestFields::direction`], and [`RequestFields::revision`]).
//! Values are written as the document is, by its canonical writer (M2.1's D7), or on one
//! line for a transport that takes one message a line ([`Layout::OneLine`]): a feature by the
//! document's own feature writer ([`Json::feature`]), a value of the document's as it holds
//! it ([`Json::plain`]; a feature's field as the document lays it out, [`Json::field`]), and
//! a request's value as it was read ([`Json::node`], taken whole by
//! [`RequestFields::node`] for the document's reader to read).
//!
//! The builders and readers are public for the core's command modules, which build their
//! wire types on them. A client (the CLI, MCP, the app) reaches commands only through the
//! core: it runs a command and writes the envelope the core gives, never building a
//! request's or a result's JSON of its own.
//!
//! Each command's wire types live in a module of their own here, beside it: its request,
//! read with [`RequestFields::read`]; its result's [`ToWire`]; and the mirror structs its
//! snapshot is generated from, each with `#[schemars(transform = wire_shape)]`, a name an
//! agent reads (`#[schemars(rename = "...")]`), and doc comments written for agents, since
//! each becomes its schema's description. Each command's snapshot is
//! `{"request": S, "result": S}`, two standalone root schemas, each with its own `$defs`:
//! the request's, which is what a call's arguments are (MCP's `inputSchema`); and the
//! result's, which is the envelope's `result` when the command runs, not the whole envelope,
//! so not by itself what MCP's `outputSchema` checks `structuredContent`, the envelope,
//! against (that is M4.9's to build from these and the envelope's). Each is added with the
//! command's wire types ([`COMMANDS`]: [`session`], for `version`, `open`, `close`,
//! `status`, and `cancel`; [`transaction`], for `begin`, `commit`, `rollback`, and `format`;
//! [`edits`], for `add_feature`, `set_field`, `remove_feature`, `replace_text`, and
//! `move_feature`; [`reads`], for `text`, `outline`, `check`, and `timeline`; [`cache`], for `cache_clear`;
//! [`elements`], for `elements` and `query`; [`schema`]; [`measure`], for `mass_properties`,
//! `distance`, and `angle`; [`render`]; [`export`]; [`sketch`]; and [`sketch_edits`], for
//! `sketch_add`, `sketch_remove`, `sketch_split`, `sketch_trim`, `sketch_unjoin`, and
//! `sketch_merge`).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

use schemars::JsonSchema;

use super::{
    Command, ModelChoice, ModelRead, Problems, Response, Revision, SessionId, SessionInfo,
};
use crate::document::json::{Member, Node, Plain, Value};
use crate::document::write::{self, Out, text};
use crate::document::{
    Angle, Coordinate, ElementId, ElementKind, ElementName, Feature, FeatureId, Length, Measure,
    NumberError, PROBLEMS_LISTED, Position, Sketch,
};
use crate::warnings::Warnings;
use crate::{Error, ErrorCode, Said, Warning, WarningCode};

pub use crate::document::write::{Layout, TooLong};

/// The most bytes a response's text may be (the design's section 6, *Limits*): 128 MiB.
/// [`response_text`] answers a response past it with a refusal.
pub const MAX_RESPONSE_LEN: usize = 128 << 20;

/// A JSON value to write, built by a [`ToWire`]: opaque, with no constructor but the
/// builders here, so every value is written as the canonical writer writes it.
pub struct Json<'a>(Out<'a>);

impl<'a> Json<'a> {
    pub fn null() -> Self {
        Json(null_out())
    }

    pub fn bool(b: bool) -> Self {
        Json(write::boolean(b))
    }

    /// A whole number.
    pub fn whole(n: u64) -> Self {
        Json(Out::Raw(n.to_string()))
    }

    /// A count, or a place counted from 0: a whole number.
    pub fn count(n: usize) -> Self {
        Json(Out::Raw(n.to_string()))
    }

    /// An integer, of either sign (a sketch's degrees of freedom).
    pub fn integer(n: i64) -> Self {
        Json(Out::Raw(n.to_string()))
    }

    pub fn str(text: &'a str) -> Self {
        Json(Out::Str(text))
    }

    pub fn string(text: String) -> Self {
        Json(Out::Owned(text))
    }

    /// A length: millimetres, a plain decimal of at most 6 places, as the document writes
    /// one.
    pub fn length(length: Length) -> Self {
        Json(write::number(length))
    }

    /// An angle: degrees, a plain decimal of at most 9 places.
    pub fn angle(angle: Angle) -> Self {
        Json(write::angle(angle))
    }

    /// An area or a volume: a plain decimal of at most 6 places, as a fingerprint writes
    /// one.
    pub fn measure(measure: Measure) -> Self {
        Json(write::measure(measure))
    }

    /// A value worked out in floating point, as a sketch's coordinate is written: the
    /// shortest decimal that reads back as the same `f64`, never an exponent, `-0` as
    /// itself. A value that isn't finite has no JSON, and is written `null`: none reaches a
    /// result, since the core refuses non-finite geometry from the kernel.
    pub fn coordinate(value: f64) -> Self {
        Json(Coordinate::new(value).map_or_else(null_out, write::coordinate))
    }

    /// A direction (a normal, an axis): each component as a [`coordinate`](Json::coordinate)
    /// is written, but `-0` written `0`.
    pub fn direction(direction: [f64; 3]) -> Self {
        Json(Out::Array(
            direction
                .iter()
                .map(|&c| Json::coordinate(if c == 0.0 { 0.0 } else { c }).0)
                .collect(),
        ))
    }

    /// A point's three lengths, `[x, y, z]`, as a feature's position is written.
    pub fn point(point: Position) -> Self {
        Json(write::point(&point))
    }

    pub fn array(items: impl IntoIterator<Item = Json<'a>>) -> Self {
        Json(Out::Array(items.into_iter().map(|j| j.0).collect()))
    }

    /// An object, its members in the order given: schema order, never sorted.
    pub fn object(members: impl IntoIterator<Item = (&'static str, Json<'a>)>) -> Self {
        Json(Out::Object(
            members.into_iter().map(|(key, j)| (key, j.0)).collect(),
        ))
    }

    /// `value`'s JSON, or `null` for none.
    pub fn or_null<T>(value: Option<T>, f: impl FnOnce(T) -> Json<'a>) -> Self {
        value.map_or_else(Json::null, f)
    }

    /// A JSON value Silico didn't build field by field (a JSON Schema, for `schema`), as the
    /// writer writes it: each object's keys sorted, numbers as serde_json writes them, and
    /// strings escaped as every string is.
    pub fn value(value: &'a serde_json::Value) -> Self {
        Json(walked(value))
    }

    /// A value of a document's, as a command gives it back (`set_field`'s field before and
    /// after, through [`field`](Json::field)): each object's members in their order, each number as its text is written,
    /// and strings escaped as every string is.
    pub fn plain(value: &'a Plain) -> Self {
        Json(walked(value))
    }

    /// A feature's field `field` (a path, as `set_field` names one), `value` its value
    /// before or after an edit, laid out as the document's canonical form lays it out: a
    /// list as [`write::field_list`] lays out that field's (a fillet's or a chamfer's
    /// `edges` a name to a line, as [`feature`](Json::feature) writes them); any other value
    /// as [`plain`](Json::plain) writes it.
    pub fn field(field: &str, value: &'a Plain) -> Self {
        match value {
            Plain::Array(items) => {
                Json(write::field_list(field, items.iter().map(walked).collect()))
            }
            _ => Json::plain(value),
        }
    }

    /// A request's value as it was read (`add_feature`'s feature, `set_field`'s value),
    /// written as [`plain`](Json::plain) writes its value.
    pub fn node(node: &'a Node) -> Self {
        Json(walked(node))
    }

    /// Features' IDs, in order.
    pub fn ids(ids: &'a [FeatureId]) -> Self {
        Json::array(ids.iter().map(ToWire::to_wire))
    }

    /// A feature as the document's canonical form writes it, by the document's own writer.
    pub fn feature(feature: &'a Feature) -> Self {
        Json(write::feature(feature))
    }

    /// Each of `sketch`'s lists, by kind in the format's order (points, lines, circles,
    /// arcs, constraints), as the document's own writer writes it (each by ID, each member's
    /// fields in the file's order), and after each member's fields, `extra`'s for it: the
    /// `sketch` read's members with their states.
    pub fn sketch_lists(
        sketch: &'a Sketch,
        extra: impl Fn(ElementKind, &ElementId) -> Vec<(&'static str, Json<'a>)>,
    ) -> [(ElementKind, Json<'a>); 5] {
        write::sketch_members(sketch).map(|(kind, items)| {
            let items = items
                .into_iter()
                .map(|(id, item)| match item {
                    Out::Object(mut members) => {
                        members.extend(extra(kind, id).into_iter().map(|(k, j)| (k, j.0)));
                        Out::Object(members)
                    }
                    other => other,
                })
                .collect();
            (kind, Json(Out::Array(items)))
        })
    }
}

/// `null`.
fn null_out<'a>() -> Out<'a> {
    Out::Raw("null".to_owned())
}

/// A JSON value as one of its kinds, its members and items as they are: how [`walked`]
/// writes a document's value ([`Plain`]), a request's ([`Node`]), and a JSON Schema's
/// (`serde_json::Value`, its object's keys sorted), alike.
enum Shape<'a, T> {
    Null,
    Bool(bool),
    Number(Cow<'a, str>),
    String(&'a str),
    Array(&'a [T]),
    Object(Vec<(&'a str, &'a T)>),
}

/// A JSON value [`walked`] writes.
trait Walk: Sized {
    fn shape(&self) -> Shape<'_, Self>;
}

impl Walk for Plain {
    fn shape(&self) -> Shape<'_, Self> {
        match self {
            Plain::Null => Shape::Null,
            Plain::Bool(b) => Shape::Bool(*b),
            Plain::Number(text) => Shape::Number(Cow::Borrowed(text)),
            Plain::String(text) => Shape::String(text),
            Plain::Array(items) => Shape::Array(items),
            Plain::Object(members) => Shape::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.as_str(), value))
                    .collect(),
            ),
        }
    }
}

impl Walk for Node {
    fn shape(&self) -> Shape<'_, Self> {
        match &self.value {
            Value::Null => Shape::Null,
            Value::Bool(b) => Shape::Bool(*b),
            Value::Number(text) => Shape::Number(Cow::Borrowed(text)),
            Value::String(text) => Shape::String(text),
            Value::Array(items) => Shape::Array(items),
            Value::Object(members) => Shape::Object(
                members
                    .iter()
                    .map(|member| (member.key.as_str(), &member.value))
                    .collect(),
            ),
        }
    }
}

impl Walk for serde_json::Value {
    fn shape(&self) -> Shape<'_, Self> {
        match self {
            serde_json::Value::Null => Shape::Null,
            serde_json::Value::Bool(b) => Shape::Bool(*b),
            serde_json::Value::Number(n) => Shape::Number(Cow::Owned(n.to_string())),
            serde_json::Value::String(s) => Shape::String(s),
            serde_json::Value::Array(items) => Shape::Array(items),
            serde_json::Value::Object(map) => {
                let mut members: Vec<(&str, &serde_json::Value)> = map
                    .iter()
                    .map(|(key, value)| (key.as_str(), value))
                    .collect();
                members.sort_by(|a, b| a.0.cmp(b.0));
                Shape::Object(members)
            }
        }
    }
}

/// `value` as the writer writes it: each object's members in the order its [`Shape`] gives
/// them, each number as its text, and strings escaped as every string is.
fn walked<T: Walk>(value: &T) -> Out<'_> {
    match value.shape() {
        Shape::Null => null_out(),
        Shape::Bool(b) => write::boolean(b),
        Shape::Number(text) => Out::Raw(text.into_owned()),
        Shape::String(text) => Out::Str(text),
        Shape::Array(items) => Out::Array(items.iter().map(walked).collect()),
        Shape::Object(members) => Out::Keyed(
            members
                .into_iter()
                .map(|(key, value)| (key.to_owned(), walked(value)))
                .collect(),
        ),
    }
}

/// `value` as text, in `layout`, if it's at most [`MAX_RESPONSE_LEN`] bytes: written only
/// so far as the limit, never past it.
pub fn write(value: &Json<'_>, layout: Layout) -> Result<String, TooLong> {
    text(&value.0, layout, MAX_RESPONSE_LEN)
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

impl ToWire for FeatureId {
    /// Its 8 characters.
    fn to_wire(&self) -> Json<'_> {
        Json::str(self.as_str())
    }
}

impl ToWire for ElementName {
    /// Its text, as written.
    fn to_wire(&self) -> Json<'_> {
        Json::str(self.as_str())
    }
}

impl ToWire for ModelChoice {
    fn to_wire(&self) -> Json<'_> {
        Json::str(self.as_str())
    }
}

impl ToWire for SessionInfo {
    /// Its ID, its path as opened (UTF-8: `open` refuses any other), its revision, and
    /// whether a transaction is open and its file was read again.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("id", self.id.to_wire()),
            ("path", Json::str(&self.path)),
            ("revision", self.revision.to_wire()),
            ("transaction", Json::bool(self.transaction)),
            ("reloaded", Json::bool(self.reloaded)),
        ])
    }
}

impl ToWire for ModelRead {
    /// Its revision, whether it was the current or the published, and the kernel fault its
    /// rebuild met, if it met one, kept as the revision's outcome (decision 32): its
    /// suggestion the worker's own, if it has one, then how a session's client tries it
    /// again, `check` with `fresh` (`fault_suggestion`, the design's section 6).
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("revision", self.revision.to_wire()),
            ("of", self.of.to_wire()),
            (
                "fault",
                Json::or_null(self.fault.as_ref(), |fault| {
                    error_with(
                        fault,
                        Json::string(super::session::fault_suggestion(
                            fault,
                            super::session::TryAgainBy::CheckWithFresh,
                        )),
                    )
                }),
            ),
        ])
    }
}

impl ToWire for Error {
    /// The error contract's shape (`tests/contract/error-full.json`), field for field.
    fn to_wire(&self) -> Json<'_> {
        error_with(self, Json::or_null(self.suggestion(), Json::str))
    }
}

/// `error` in the error contract's shape, `suggestion` its suggestion.
fn error_with<'a>(error: &'a Error, suggestion: Json<'a>) -> Json<'a> {
    said_as(error.said(), suggestion)
}

impl ToWire for Warning {
    /// The error contract's shape, as an error's, its code a warning's (decision 17).
    fn to_wire(&self) -> Json<'_> {
        said_as(self.said(), Json::or_null(self.suggestion(), Json::str))
    }
}

/// What an error or a warning says (`said`, taken whole, so a field it gains is one this
/// must write) in the error contract's shape (`tests/contract/error-full.json`), in its
/// order, `suggestion` written as its suggestion (a fault's, kept, is said its own way).
fn said_as<'a>(said: Said<'a>, suggestion: Json<'a>) -> Json<'a> {
    let Said {
        code,
        message,
        feature,
        elements,
        elements_total,
        parameter,
        cause,
        suggestion: _,
        candidates,
        advice,
    } = said;
    Json::object([
        ("code", Json::str(code)),
        ("message", Json::str(message)),
        ("feature", Json::or_null(feature, Json::str)),
        (
            "elements",
            Json::array(elements.iter().map(|e| Json::str(e))),
        ),
        ("elements_total", Json::whole(elements_total)),
        ("parameter", Json::or_null(parameter, Json::str)),
        ("cause", Json::or_null(cause, Json::str)),
        ("suggestion", suggestion),
        (
            "candidates",
            Json::array(candidates.iter().map(|c| Json::str(c))),
        ),
        ("advice", Json::or_null(advice, Json::str)),
    ])
}

/// A list of warnings as `check`'s result writes them: the first [`PROBLEMS_LISTED`], in the
/// error contract's shape, and how many there are, listed or not (`warnings` and
/// `warnings_total`).
pub fn warnings(warnings: &Warnings) -> (Json<'_>, Json<'_>) {
    listed(warnings.listed(), warnings.total())
}

/// The first [`PROBLEMS_LISTED`] of `items`, in the error contract's shape, and `total`, how
/// many there are, listed or not: the one rule for problems and warnings alike.
fn listed<T: ToWire>(items: &[T], total: u64) -> (Json<'_>, Json<'_>) {
    (
        Json::array(items.iter().take(PROBLEMS_LISTED).map(ToWire::to_wire)),
        Json::whole(total),
    )
}

/// A list of problems as a result or the envelope writes them: the first
/// [`PROBLEMS_LISTED`] errors, in the error contract's shape, and how many there are,
/// listed or not (`problems` and `problems_total` in a result, `errors` and `total` in a
/// refusal).
pub fn problems(problems: &Problems) -> (Json<'_>, Json<'_>) {
    listed(problems.errors(), problems.total())
}

/// A command's response, in its envelope (section 2): `ok`, the session as the command left
/// it, the model it read, and its result; or, refused, as [`refused`] writes it.
pub fn response<T: ToWire>(response: &Response<T>) -> Json<'_> {
    match &response.result {
        Ok(result) => Json::object([
            ("ok", Json::bool(true)),
            (
                "session",
                Json::or_null(response.session.as_ref(), ToWire::to_wire),
            ),
            (
                "model",
                Json::or_null(response.model.as_ref(), ToWire::to_wire),
            ),
            ("result", result.to_wire()),
        ]),
        Err(problems) => refused(response.session.as_ref(), response.model.as_ref(), problems),
    }
}

/// A refusal's envelope (section 2): `ok: false`, the session and the model, if any, and
/// every reason, the first [`PROBLEMS_LISTED`], and how many there are. A command's refusal
/// is its [`response`]'s; this is also every envelope that isn't a command's (MCP's
/// JSON-RPC errors, whose `data` is one).
pub fn refused<'a>(
    session: Option<&'a SessionInfo>,
    model: Option<&'a ModelRead>,
    problems: &'a Problems,
) -> Json<'a> {
    let (errors, total) = self::problems(problems);
    Json::object([
        ("ok", Json::bool(false)),
        ("session", Json::or_null(session, ToWire::to_wire)),
        ("model", Json::or_null(model, ToWire::to_wire)),
        ("errors", errors),
        ("total", total),
    ])
}

/// A command's response as text, in `layout`: its envelope; or, if that would be more than
/// [`MAX_RESPONSE_LEN`] bytes, a refusal saying so, with the same session and model
/// (`invalid_input`, section 6's *Limits*: a command whose result can pass it names the
/// field that makes it before it answers).
pub fn response_text<T: ToWire>(response: &Response<T>, layout: Layout) -> String {
    response_text_within(response, layout, MAX_RESPONSE_LEN)
}

/// [`response_text`], against `limit`.
fn response_text_within<T: ToWire>(response: &Response<T>, layout: Layout, limit: usize) -> String {
    answer_within(response, layout, limit).1
}

/// [`response_text`], and whether what it wrote is the answer of a command that ran (its
/// `ok`): false for a refusal, and for a response refused as too long to write.
pub(crate) fn answer_text<T: ToWire>(response: &Response<T>, layout: Layout) -> (bool, String) {
    answer_within(response, layout, MAX_RESPONSE_LEN)
}

/// [`answer_text`], against `limit`.
fn answer_within<T: ToWire>(
    response: &Response<T>,
    layout: Layout,
    limit: usize,
) -> (bool, String) {
    let _span = tracing::debug_span!("response_text", ?layout).entered();
    if let Ok(written) = text(&self::response(response).0, layout, limit) {
        return (response.result.is_ok(), written);
    }
    let too_long = Problems::from(
        Error::new(
            ErrorCode::InvalidInput,
            format!("the response would be more than the {limit} bytes a response may be"),
        )
        .with_cause(
            "a command that changes the document may have done its work before its answer was \
             written: read the session's revision before asking again",
        )
        .with_suggestion("ask for less at once"),
    );
    let session = response.session.as_ref();
    let model = response.model.as_ref();
    let refusal = text(&refused(session, model, &too_long).0, layout, limit)
        // A refusal too long itself (a session's path near the limit, which no path is) is
        // written without its session and model: some hundred bytes, written in full.
        .or_else(|TooLong| text(&refused(None, None, &too_long).0, layout, usize::MAX))
        // Never empty: with no limit, writing can't fail.
        .unwrap_or_default();
    (false, refusal)
}

/// The refusals a request's reader keeps: the first [`PROBLEMS_LISTED`], in the order found,
/// and how many more there are. A refusal past the hundredth is only counted, never made,
/// so a request of a million bad values costs a hundred errors, not a million.
#[derive(Default)]
struct Refusals {
    listed: Vec<Error>,
    unlisted: u64,
}

impl Refusals {
    /// The refusal `error` makes, kept if fewer than [`PROBLEMS_LISTED`] are, and counted
    /// either way: made only if it's kept.
    fn refuse_with(&mut self, error: impl FnOnce() -> Error) {
        if self.listed.len() < PROBLEMS_LISTED {
            self.listed.push(error());
        } else {
            self.unlisted += 1;
        }
    }

    /// `more`'s refusals after these, as if they'd been found here.
    fn extend(&mut self, more: Refusals) {
        for error in more.listed {
            self.refuse_with(|| error);
        }
        self.unlisted += more.unlisted;
    }

    fn is_empty(&self) -> bool {
        self.listed.is_empty() && self.unlisted == 0
    }
}

/// A request's fields, read strictly (section 2's *Requests are strict*), through
/// [`RequestFields::read`]: each field taken by name and of its type, every field the shape
/// gives required, `null` only where the shape allows it, and, at the end, any key not taken
/// refused, at the top and in every object read with [`object`](RequestFields::object) or
/// [`nullable_object`](RequestFields::nullable_object). Every reason is kept, in the order
/// found, each an `invalid_input` error naming its field by its dotted path (decision 15:
/// here, a member of a list by its place, from 0), saying what the field must be and what it
/// was ("limit must be a whole number from 0 to 4294967295, got 1.5").
///
/// Each reader gives `None` for a field it refused, so a request's reader gives `None` if
/// any is: a value is had only from a request with nothing refused.
pub struct RequestFields<'a> {
    /// Where these fields lie in the request: empty at its top.
    path: String,
    members: &'a [Member],
    taken: Vec<bool>,
    refused: Refusals,
}

impl<'a> RequestFields<'a> {
    /// The request `node`, which must be an object, read by `read`: what it gives, or every
    /// reason it was refused, `read`'s and every key it didn't take, the first
    /// [`PROBLEMS_LISTED`] listed and all counted. `read` giving nothing though nothing was
    /// refused is a bug, said as one.
    pub fn read<T>(
        node: &'a Node,
        read: impl FnOnce(&mut RequestFields<'a>) -> Option<T>,
    ) -> Result<T, Problems> {
        let mut fields = RequestFields::of(node, String::new())?;
        let value = read(&mut fields);
        fields.done()?;
        value.ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "a request's reader gave no value, but refused nothing",
            )
            .with_suggestion(crate::worker::SILICO_BUG)
            .into()
        })
    }

    /// The fields of `node`, which must be an object, at `path` in the request.
    fn of(node: &'a Node, path: String) -> Result<Self, Error> {
        match &node.value {
            Value::Object(members) => Ok(RequestFields {
                path,
                members,
                taken: vec![false; members.len()],
                refused: Refusals::default(),
            }),
            other => Err(invalid(
                &path,
                format!(
                    "{} must be an object, got {}",
                    the_field(&path),
                    other.kind()
                ),
            )),
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
                self.refused.refuse_with(|| {
                    let at = join(&self.path, key);
                    invalid(&at, format!("{at} is missing")).with_suggestion(
                        "give every field the request's shape has; null where it allows none",
                    )
                });
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
    fn wrong(&mut self, key: &str, what: impl fmt::Display, node: &Node) {
        self.refused.refuse_with(|| {
            let at = join(&self.path, key);
            invalid(&at, format!("{at} must be {what}, got {}", written(node)))
        });
    }

    /// Refuses the request for `error`, a reason no one field's reading gives (fields that
    /// must agree), kept with the rest in the order found.
    pub fn refuse(&mut self, error: Error) {
        self.refused.refuse_with(|| error);
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

    /// `true`, `false`, or `null`.
    pub fn nullable_bool(&mut self, key: &str) -> Option<Option<bool>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => match node.value {
                Value::Bool(b) => Some(Some(b)),
                _ => {
                    self.wrong(key, "true, false, or null", node);
                    None
                }
            },
        }
    }

    /// One of `words`, each a word and what it reads as. Refused, any other value is said
    /// as written if it's a plain word (`got solid`), else quoted and escaped, the words
    /// given as candidates, suggesting one of `what` (`kinds`).
    pub fn word<T: Copy>(
        &mut self,
        key: &str,
        words: &[(&'static str, T)],
        what: &str,
    ) -> Option<T> {
        let node = self.take(key)?;
        self.word_of(key, node, words, what, false)
    }

    /// One of `words`, as [`word`](RequestFields::word) reads it, or `null`.
    pub fn nullable_word<T: Copy>(
        &mut self,
        key: &str,
        words: &[(&'static str, T)],
        what: &str,
    ) -> Option<Option<T>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self.word_of(key, node, words, what, true).map(Some),
        }
    }

    /// `node`, `key`'s value, as one of `words`, or, refused, said to take `null` too if
    /// it's `nullable`.
    fn word_of<T: Copy>(
        &mut self,
        key: &str,
        node: &Node,
        words: &[(&'static str, T)],
        what: &str,
        nullable: bool,
    ) -> Option<T> {
        let found = match &node.value {
            Value::String(text) => words.iter().find(|(word, _)| word == text),
            _ => None,
        };
        if let Some((_, value)) = found {
            return Some(*value);
        }
        self.refused.refuse_with(|| {
            let at = join(&self.path, key);
            let got = match &node.value {
                // A plain word bare (`got solid`), but quoted if it spells a JSON value
                // (`"null"`, `"3"`), so it reads apart from that value.
                Value::String(text) if !spells_a_value(text) => crate::document::field_name(text),
                _ => written(node),
            };
            let names: Vec<&str> = words.iter().map(|(word, _)| *word).collect();
            let mut choices = names.clone();
            if nullable {
                choices.push("null");
            }
            invalid(
                &at,
                format!("{at} must be {}, got {got}", alternatives(&choices)),
            )
            .with_suggestion(format!("give one of the {what}"))
            .with_candidates(names.iter().map(|&name| name.to_owned()))
        });
        None
    }

    /// A whole number from `low` to `high`, written as its digits alone: no sign,
    /// fraction, exponent, or leading zero.
    fn whole_in(&mut self, key: &str, low: u64, high: u64) -> Option<u64> {
        let node = self.take(key)?;
        self.whole_of(key, node, low, high, "")
    }

    /// `node`, `key`'s value, as [`whole_in`](RequestFields::whole_in) reads it, said to
    /// take `or`, too (` or null`), if it's refused.
    fn whole_of(&mut self, key: &str, node: &Node, low: u64, high: u64, or: &str) -> Option<u64> {
        let whole = match &node.value {
            Value::Number(text) => super::whole_number(text).filter(|n| (low..=high).contains(n)),
            _ => None,
        };
        if whole.is_none() {
            self.wrong(
                key,
                format_args!("a whole number from {low} to {high}{or}"),
                node,
            );
        }
        whole
    }

    /// A whole number from 0 to `u64::MAX`, or `null`.
    pub fn nullable_u64(&mut self, key: &str) -> Option<Option<u64>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self.whole_of(key, node, 0, u64::MAX, " or null").map(Some),
        }
    }

    /// A whole number from 0 to `u32::MAX`.
    pub fn u32(&mut self, key: &str) -> Option<u32> {
        self.whole_in(key, 0, u32::MAX.into())
            .and_then(|n| u32::try_from(n).ok())
    }

    /// A session's revision: a whole number from 1 to `u64::MAX`.
    pub fn revision(&mut self, key: &str) -> Option<Revision> {
        self.whole_in(key, 1, u64::MAX)
            .and_then(Revision::from_number)
    }

    /// A length, in millimetres: read exactly from its digits, and refused as a
    /// document's is (a seventh decimal, `-0`, past an `i64` of nanometres).
    pub fn length(&mut self, key: &str) -> Option<Length> {
        let node = self.take(key)?;
        self.quantity(
            key,
            node,
            "a length in millimetres",
            Length::parse,
            crate::document::length_suggestion,
        )
    }

    /// A length, or `null`.
    pub fn nullable_length(&mut self, key: &str) -> Option<Option<Length>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self
                .quantity(
                    key,
                    node,
                    "a length in millimetres or null",
                    Length::parse,
                    crate::document::length_suggestion,
                )
                .map(Some),
        }
    }

    /// An angle, in degrees: read exactly from its digits, and refused as a document's is
    /// (a tenth decimal, `-0`, past an `i64` of nanodegrees).
    pub fn angle(&mut self, key: &str) -> Option<Angle> {
        let node = self.take(key)?;
        self.quantity(
            key,
            node,
            "an angle in degrees",
            Angle::parse,
            crate::document::angle_suggestion,
        )
    }

    /// An angle, or `null`.
    pub fn nullable_angle(&mut self, key: &str) -> Option<Option<Angle>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self
                .quantity(
                    key,
                    node,
                    "an angle in degrees or null",
                    Angle::parse,
                    crate::document::angle_suggestion,
                )
                .map(Some),
        }
    }

    /// An area, in square millimetres, or `null`: read exactly from its digits, to at most
    /// 6 decimals, as a fingerprint's is.
    pub fn nullable_area(&mut self, key: &str) -> Option<Option<Measure>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self
                .quantity(
                    key,
                    node,
                    "an area in square millimetres or null",
                    Measure::parse,
                    crate::document::area_suggestion,
                )
                .map(Some),
        }
    }

    /// `node`, `key`'s value, as a quantity `parse` reads from its digits: `what` it must
    /// be, and, refused, what to do (`suggestion`), as a document's refusal says it.
    fn quantity<Q>(
        &mut self,
        key: &str,
        node: &Node,
        what: &str,
        parse: fn(&str) -> Result<Q, NumberError>,
        suggestion: fn(&NumberError) -> String,
    ) -> Option<Q> {
        let Value::Number(text) = &node.value else {
            self.wrong(key, what, node);
            return None;
        };
        match parse(text) {
            Ok(value) => Some(value),
            Err(e) => {
                self.refused.refuse_with(|| {
                    let at = join(&self.path, key);
                    invalid(
                        &at,
                        format!(
                            "{at} must be {what}, got {}, which {e}",
                            crate::document::shown_number(text)
                        ),
                    )
                    .with_suggestion(suggestion(&e))
                });
                None
            }
        }
    }

    /// A direction (a normal, an axis): three finite numbers, `[x, y, z]`, each read
    /// correctly rounded to the nearest `f64`, a member at fault named by its place
    /// (`normal.2`).
    pub fn direction(&mut self, key: &str) -> Option<[f64; 3]> {
        let node = self.take(key)?;
        let what = "a direction, three numbers [x, y, z]";
        let Value::Array(items) = &node.value else {
            self.wrong(key, what, node);
            return None;
        };
        let [x, y, z] = items.as_slice() else {
            self.refused.refuse_with(|| {
                let at = join(&self.path, key);
                invalid(
                    &at,
                    format!("{at} must be {what}, got {} of them", items.len()),
                )
            });
            return None;
        };
        let path = &self.path;
        let refused = &mut self.refused;
        let mut component = |i: usize, item: &Node| {
            let value = match &item.value {
                Value::Number(text) => Coordinate::parse(text).map(Coordinate::mm),
                _ => None,
            };
            if value.is_none() {
                refused.refuse_with(|| {
                    let at = format!("{}.{i}", join(path, key));
                    invalid(
                        &at,
                        format!("{at} must be a finite number, got {}", written(item)),
                    )
                });
            }
            value
        };
        // Each read, so each member at fault is said.
        let (x, y, z) = (component(0, x), component(1, y), component(2, z));
        let direction = [x?, y?, z?];
        // A direction points somewhere: not all zero. (Any other finite vector has a
        // direction, however large or small, if scaled by its largest component first.)
        if direction.iter().all(|&c| c == 0.0) {
            self.refused.refuse_with(|| {
                let at = join(&self.path, key);
                invalid(&at, format!("{at} must be a direction, got [0, 0, 0]"))
                    .with_suggestion("give a direction that isn't zero")
            });
            return None;
        }
        Some(direction)
    }

    /// A point, in millimetres: three lengths, `[x, y, z]`, each read exactly from its
    /// digits as [`length`](RequestFields::length) reads one, a member at fault named by
    /// its place (`eye.2`).
    pub fn point(&mut self, key: &str) -> Option<[Length; 3]> {
        let node = self.take(key)?;
        let what = "a point, three lengths in millimetres [x, y, z]";
        let Value::Array(items) = &node.value else {
            self.wrong(key, what, node);
            return None;
        };
        let [x, y, z] = items.as_slice() else {
            self.refused.refuse_with(|| {
                let at = join(&self.path, key);
                invalid(
                    &at,
                    format!("{at} must be {what}, got {} of them", items.len()),
                )
            });
            return None;
        };
        // Each read, so each member at fault is said.
        let x = self.quantity(
            &format!("{key}.0"),
            x,
            "a length in millimetres",
            Length::parse,
            crate::document::length_suggestion,
        );
        let y = self.quantity(
            &format!("{key}.1"),
            y,
            "a length in millimetres",
            Length::parse,
            crate::document::length_suggestion,
        );
        let z = self.quantity(
            &format!("{key}.2"),
            z,
            "a length in millimetres",
            Length::parse,
            crate::document::length_suggestion,
        );
        Some([x?, y?, z?])
    }

    /// The list at `key` of at most `at_most` strings, a member at fault named by its place
    /// (`highlight.0`). A list of more is refused as `too_many` says, given how many it
    /// holds, and its members aren't read.
    pub fn string_list(
        &mut self,
        key: &str,
        at_most: usize,
        too_many: impl FnOnce(usize) -> Error,
    ) -> Option<Vec<String>> {
        let node = self.take(key)?;
        let Value::Array(items) = &node.value else {
            self.wrong(key, "a list of strings", node);
            return None;
        };
        if items.len() > at_most {
            self.refused.refuse_with(|| too_many(items.len()));
            return None;
        }
        let mut strings = Vec::with_capacity(items.len());
        let mut fine = true;
        for (i, item) in items.iter().enumerate() {
            match &item.value {
                Value::String(text) => strings.push(text.clone()),
                _ => {
                    fine = false;
                    self.refused.refuse_with(|| {
                        let at = format!("{}.{i}", join(&self.path, key));
                        invalid(&at, format!("{at} must be a string, got {}", written(item)))
                    });
                }
            }
        }
        fine.then_some(strings)
    }

    /// The session a request names: its ID, `s1`, `s2`, ...
    pub fn session(&mut self) -> Option<SessionId> {
        let node = self.take("session")?;
        let id = match &node.value {
            Value::String(text) => SessionId::parse(text),
            _ => None,
        };
        if id.is_none() {
            self.refused.refuse_with(|| {
                let at = join(&self.path, "session");
                invalid(
                    &at,
                    format!(
                        "{at} must be a session's ID, s1, s2, ..., got {}",
                        written(node)
                    ),
                )
                .with_suggestion("give the ID open returned")
            });
        }
        id
    }

    /// Which model a read takes (decision 33): `current` or `published`.
    pub fn model(&mut self) -> Option<ModelChoice> {
        self.word(
            "model",
            &ModelChoice::ALL.map(|c| (c.as_str(), c)),
            "models",
        )
    }

    /// A feature's ID.
    pub fn feature(&mut self, key: &str) -> Option<FeatureId> {
        let node = self.take(key)?;
        self.feature_of(key, node, "a feature's ID")
    }

    /// A feature's ID, or `null`.
    pub fn nullable_feature(&mut self, key: &str) -> Option<Option<FeatureId>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self
                .feature_of(key, node, "a feature's ID or null")
                .map(Some),
        }
    }

    /// `node`, `key`'s value, as a feature's ID: `what` it must be, refused.
    fn feature_of(&mut self, key: &str, node: &Node, what: &str) -> Option<FeatureId> {
        let id = match &node.value {
            Value::String(text) => FeatureId::new(text),
            _ => None,
        };
        if id.is_none() {
            self.wrong(key, what, node);
        }
        id
    }

    /// A list of exactly `N` strings (`what` each is: `a name`), a member at fault named by
    /// its place (`elements.1`).
    pub fn strings<const N: usize>(&mut self, key: &str, what: &str) -> Option<[String; N]> {
        let node = self.take(key)?;
        let Value::Array(items) = &node.value else {
            self.wrong(key, format_args!("a list of {N} {what}s"), node);
            return None;
        };
        if items.len() != N {
            self.refused.refuse_with(|| {
                let at = join(&self.path, key);
                invalid(
                    &at,
                    format!(
                        "{at} must be a list of {N} {what}s, got {} of them",
                        items.len()
                    ),
                )
            });
            return None;
        }
        let mut found = Vec::with_capacity(N);
        for (i, item) in items.iter().enumerate() {
            match &item.value {
                Value::String(text) => found.push(text.clone()),
                _ => self.refused.refuse_with(|| {
                    let at = format!("{}.{i}", join(&self.path, key));
                    invalid(&at, format!("{at} must be a {what}, got {}", written(item)))
                }),
            }
        }
        found.try_into().ok()
    }

    /// A list of features' IDs, or `null`; a member at fault named by its place
    /// (`features.0`).
    pub fn nullable_feature_ids(&mut self, key: &str) -> Option<Option<Vec<FeatureId>>> {
        let node = match self.take_nullable(key)? {
            None => return Some(None),
            Some(node) => node,
        };
        let Value::Array(items) = &node.value else {
            self.wrong(key, "a list of features' IDs or null", node);
            return None;
        };
        let mut ids = Vec::with_capacity(items.len());
        let mut fine = true;
        for (i, item) in items.iter().enumerate() {
            match &item.value {
                Value::String(text) if let Some(id) = FeatureId::new(text) => ids.push(id),
                _ => {
                    fine = false;
                    self.refused.refuse_with(|| {
                        let at = format!("{}.{i}", join(&self.path, key));
                        invalid(
                            &at,
                            format!("{at} must be a feature's ID, got {}", written(item)),
                        )
                    });
                }
            }
        }
        fine.then_some(Some(ids))
    }

    /// The list at `key`, of at most `at_most` objects, each read by `read` as the request
    /// is, a member named by its place (`measures.0`). A list of more is refused as
    /// `too_many` says, given how many it holds, and its members aren't read, so a list of a
    /// million costs one refusal.
    pub fn objects<T>(
        &mut self,
        key: &str,
        at_most: usize,
        too_many: impl FnOnce(usize) -> Error,
        mut read: impl FnMut(&mut RequestFields<'a>) -> Option<T>,
    ) -> Option<Vec<T>> {
        let node = self.take(key)?;
        let Value::Array(items) = &node.value else {
            self.wrong(key, "a list", node);
            return None;
        };
        if items.len() > at_most {
            self.refused.refuse_with(|| too_many(items.len()));
            return None;
        }
        let mut values = Vec::with_capacity(items.len());
        let mut fine = true;
        for (i, item) in items.iter().enumerate() {
            match self.nested(&format!("{key}.{i}"), item, &mut read) {
                Some(value) => values.push(value),
                None => fine = false,
            }
        }
        fine.then_some(values)
    }

    /// The value of `key`, whatever it is, taken as it is: for a field another reader
    /// reads (`set_field`'s value, read as the document reads it), or one left unread when
    /// what decides its reading was refused (a range's bounds, read as its measure's
    /// quantity). Refused only if it's missing.
    pub fn node(&mut self, key: &str) -> Option<&'a Node> {
        self.take(key)
    }

    /// The object at `key`, taken whole, its members unread: for an object another reader
    /// reads (`add_feature`'s feature, read as the document reads a feature). Refused if it's
    /// missing or isn't an object, as [`object`](RequestFields::object) refuses one.
    pub fn whole_object(&mut self, key: &str) -> Option<&'a Node> {
        let node = self.take(key)?;
        match RequestFields::of(node, join(&self.path, key)) {
            Ok(_) => Some(node),
            Err(refused) => {
                self.refused.refuse_with(|| refused);
                None
            }
        }
    }

    /// The object at `key`, read by `read` as the request is: its refusals, and any of its
    /// keys `read` didn't take, kept with these. What `read` gives, if nothing in it was
    /// refused.
    pub fn object<T>(
        &mut self,
        key: &str,
        read: impl FnOnce(&mut RequestFields<'a>) -> Option<T>,
    ) -> Option<T> {
        let node = self.take(key)?;
        self.nested(key, node, read)
    }

    /// The object at `key`, read by `read` as [`object`](RequestFields::object) reads it, or
    /// `Some(None)` if it's `null`.
    pub fn nullable_object<T>(
        &mut self,
        key: &str,
        read: impl FnOnce(&mut RequestFields<'a>) -> Option<T>,
    ) -> Option<Option<T>> {
        match self.take_nullable(key)? {
            None => Some(None),
            Some(node) => self.nested(key, node, read).map(Some),
        }
    }

    /// `node`, the value of `key`, read by `read` as an object of its own.
    fn nested<T>(
        &mut self,
        key: &str,
        node: &'a Node,
        read: impl FnOnce(&mut RequestFields<'a>) -> Option<T>,
    ) -> Option<T> {
        let mut inner = match RequestFields::of(node, join(&self.path, key)) {
            Ok(inner) => inner,
            Err(refused) => {
                self.refused.refuse_with(|| refused);
                return None;
            }
        };
        let value = read(&mut inner);
        let clean = self.absorb(inner);
        value.filter(|_| clean)
    }

    /// Keeps `inner`'s refusals, an object read from one of these fields, with these, every
    /// key it didn't take refused: whether it had none.
    fn absorb(&mut self, inner: RequestFields<'a>) -> bool {
        let refused = inner.finish();
        let clean = refused.is_empty();
        self.refused.extend(refused);
        clean
    }

    /// Every key not taken, refused, and every refusal so far, the first
    /// [`PROBLEMS_LISTED`] listed and all counted; or `Ok` if there are none.
    fn done(self) -> Result<(), Problems> {
        let refused = self.finish();
        if refused.is_empty() {
            return Ok(());
        }
        Err(Problems::with_unlisted(refused.listed, refused.unlisted))
    }

    /// The refusals, every key not taken refused. A key is named as the document's reader
    /// names one (`field_name`): as written if it's a plain name, otherwise quoted, escaped,
    /// and cut short, so no key makes its refusal unsafe to print, long, or a path that
    /// names another field (`a.b`).
    fn finish(self) -> Refusals {
        let RequestFields {
            path,
            members,
            taken,
            mut refused,
        } = self;
        for (member, taken) in members.iter().zip(taken) {
            if !taken {
                refused.refuse_with(|| {
                    let at = join(&path, &crate::document::field_name(&member.key));
                    invalid(&at, format!("{at} isn't a field of this request")).with_suggestion(
                        "leave it out: a request has only the fields its shape gives",
                    )
                });
            }
        }
        refused
    }
}

/// Whether `text`, written bare, would read as a JSON value: a literal, or a number (any
/// word that starts as one does).
fn spells_a_value(text: &str) -> bool {
    matches!(text, "null" | "true" | "false")
        || text.starts_with(|c: char| c.is_ascii_digit() || c == '-')
}

/// `names` as a message lists the choices: `current or published`, `face, edge, or vertex`.
fn alternatives(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// The dotted path of `key` in the object at `path` (empty at the request's top).
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
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
fn the_field(path: &str) -> &str {
    if path.is_empty() { "the request" } else { path }
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

/// A wire type's schema closed to its own properties, for a container transform
/// (`#[schemars(transform = wire_shape)]`) on every wire mirror struct and enum: each of them
/// required, since a response holds every field, `null` where it has nothing (section 2),
/// where schemars leaves an `Option` field out of `required` (its type still takes `null`);
/// and no other (`"additionalProperties": false`), so a field a `ToWire` writes that its
/// mirror doesn't have fails the snapshot's validation. An enum's variants written inline
/// (`oneOf` or `anyOf` of objects, as an internally tagged or untagged enum of struct
/// variants is) are its own, and closed too; a variant that is a struct of its own (a
/// `$ref`) is closed by its own `wire_shape`. Only the type's own properties: a document type
/// a result carries (a feature) keeps the document's own schema.
pub(crate) fn wire_shape(schema: &mut schemars::Schema) {
    if let Some(schema) = schema.as_object_mut() {
        close(schema);
    }
}

/// A string schema of one of `words`, described as `description`.
fn words(description: &str, words: impl IntoIterator<Item = &'static str>) -> schemars::Schema {
    let words: Vec<&str> = words.into_iter().collect();
    schemars::json_schema!({
        "description": description,
        "type": "string",
        "enum": words,
    })
}

/// `schema`, an object's schema, closed to its properties (an object of none, a request or
/// result with no fields, to none), and each variant it has inline.
fn close(schema: &mut serde_json::Map<String, serde_json::Value>) {
    if !schema.contains_key("properties") && schema.get("type") == Some(&"object".into()) {
        schema.insert(
            "additionalProperties".to_owned(),
            serde_json::Value::Bool(false),
        );
    }
    let keys: Option<Vec<serde_json::Value>> = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| {
            let mut keys: Vec<&String> = properties.keys().collect();
            keys.sort();
            keys.into_iter()
                .map(|k| serde_json::Value::String(k.clone()))
                .collect()
        });
    if let Some(keys) = keys {
        schema.insert("required".to_owned(), serde_json::Value::Array(keys));
        schema.insert(
            "additionalProperties".to_owned(),
            serde_json::Value::Bool(false),
        );
    }
    for variants in ["oneOf", "anyOf"] {
        if let Some(serde_json::Value::Array(variants)) = schema.get_mut(variants) {
            for variant in variants {
                if let serde_json::Value::Object(variant) = variant {
                    close(variant);
                }
            }
        }
    }
}

/// An error's `code`: one of Silico's error codes, every one, in their order
/// ([`ErrorCode::ALL`], which `error-codes.txt` is checked against), so an agent finds them in
/// the schema it reads, and a code appended there is in the schema with it.
fn error_code_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    codes(ErrorCode::ALL.iter().map(|code| code.as_str()))
}

/// A warning's `code`: one of Silico's warning codes, every one, in their order
/// ([`WarningCode::ALL`], which `warning-codes.txt` is checked against).
fn warning_code_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    codes(WarningCode::ALL.iter().map(|code| code.as_str()))
}

/// A string schema of one of `codes`, in order.
fn codes(codes: impl IntoIterator<Item = &'static str>) -> schemars::Schema {
    let codes: Vec<&str> = codes.into_iter().collect();
    schemars::json_schema!({
        "type": "string",
        "enum": codes,
    })
}

/// Why a command was refused, or a problem it found, in Silico's error contract: a code, a
/// message, and, where they apply, the feature, the elements at fault (the first of them, and
/// how many in all), the request's field at fault, the cause, what to do, and the names that
/// might have been meant. Every field is always present: null, or empty, where it doesn't
/// apply.
#[derive(JsonSchema)]
#[schemars(rename = "Error", transform = wire_shape)]
pub struct ErrorSchema {
    /// What kind of error it is: one of Silico's error codes, such as `invalid_input`.
    #[schemars(schema_with = "error_code_schema")]
    pub code: String,
    /// What went wrong, in a sentence.
    pub message: String,
    /// The ID of the feature at fault, if one is.
    pub feature: Option<String>,
    /// The elements at fault, by name: the first of them, if there are many.
    pub elements: Vec<String>,
    /// How many elements are at fault, listed or not.
    pub elements_total: u64,
    /// The field at fault, as a dotted path (`position.2`), if one is.
    pub parameter: Option<String>,
    /// What lay under it, such as the kernel's own failure, if known.
    pub cause: Option<String>,
    /// What to do about it, if Silico can say.
    pub suggestion: Option<String>,
    /// What might have been meant, such as the names nearest one that found nothing.
    pub candidates: Vec<String>,
    /// What the suggestion advises, as data, for a kernel fault whose worker said:
    /// `change_the_values` (the request's values crashed the worker, and would again: change
    /// them, never retry as is), `simplify_or_wait` (it ran past its time: simplify the
    /// operation, or try again), `try_again` (it wasn't the values: try the request again),
    /// or `try_once_more` (only once more: the worker may have been stopped for the memory
    /// the request used). Null for any other error, and a fault whose advice is in its
    /// suggestion's words alone.
    #[schemars(schema_with = "advice_schema")]
    pub advice: Option<String>,
}

/// An error's or a warning's `advice`: one of the worker's advice's words, in their order
/// (`Advice::WORDS`), or null.
fn advice_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let mut words: Vec<serde_json::Value> = crate::worker::Advice::WORDS
        .iter()
        .map(|&word| serde_json::Value::from(word))
        .collect();
    words.push(serde_json::Value::Null);
    schemars::json_schema!({
        "type": ["string", "null"],
        "enum": words,
    })
}

/// Something `check` found that fails nothing: it never refuses a command, blocks a save, or
/// sets an exit code. In the error contract's shape, each field as an error's: a code, a
/// message, and, where they apply, the feature, the elements it's of (the first of them,
/// and how many in all), the field it's of, the cause, what to do, and the names that might
/// have been meant. Every field is always present: null, or empty, where it doesn't apply.
#[derive(JsonSchema)]
#[schemars(rename = "Warning", transform = wire_shape)]
pub struct WarningSchema {
    /// What kind of warning it is: one of Silico's warning codes, listed in
    /// `warning-codes.txt`, such as `ends_not_joined`.
    #[schemars(schema_with = "warning_code_schema")]
    pub code: String,
    /// What was found, in a sentence.
    pub message: String,
    /// The ID of the feature it's of, if one is.
    pub feature: Option<String>,
    /// The elements it's of, by name: the first of them, if there are many.
    pub elements: Vec<String>,
    /// How many elements it's of, listed or not.
    pub elements_total: u64,
    /// The field it's of, as a dotted path (`position.2`), if one is.
    pub parameter: Option<String>,
    /// What lay under it, if known.
    pub cause: Option<String>,
    /// What to do about it, if Silico can say.
    pub suggestion: Option<String>,
    /// What might have been meant, if anything.
    pub candidates: Vec<String>,
    /// Null: a warning is never a worker's fault, so it advises nothing as data (an error's
    /// `advice`, in the same shape).
    #[schemars(schema_with = "advice_schema")]
    pub advice: Option<String>,
}

/// The session as the command left it.
#[derive(JsonSchema)]
#[schemars(rename = "Session", transform = wire_shape)]
pub struct SessionSchema {
    /// The session's ID, `s1`, `s2`, ...: give it in each request to the session.
    pub id: String,
    /// The document's path, as `open` was given it.
    pub path: String,
    /// The session's revision: 1 at `open`, and one more for each change to its document.
    #[schemars(range(min = 1))]
    pub revision: u64,
    /// Whether a transaction is open.
    pub transaction: bool,
    /// Whether the session read its file again before the command, finding it changed on
    /// disk.
    pub reloaded: bool,
}

/// Which model was read: the session's current revision's (`current`), or the newest one
/// published (`published`), which may be an older revision's.
#[derive(JsonSchema)]
#[schemars(rename = "ModelOf", rename_all = "lowercase")]
pub enum ModelOfSchema {
    Current,
    Published,
}

/// The model the command read.
#[derive(JsonSchema)]
#[schemars(rename = "Model", transform = wire_shape)]
pub struct ModelSchema {
    /// The revision the model was rebuilt from.
    #[schemars(range(min = 1))]
    pub revision: u64,
    pub of: ModelOfSchema,
    /// The kernel fault the model's rebuild met, if it met one: null if none.
    pub fault: Option<ErrorSchema>,
}

/// A command that ran: `ok` is true, and `result` is the command's own result.
#[derive(JsonSchema)]
#[schemars(rename = "Ran", transform = wire_shape)]
pub struct RanSchema {
    #[schemars(extend("const" = true))]
    pub ok: bool,
    /// The session as the command left it: null for a command that takes no session.
    pub session: Option<SessionSchema>,
    /// The model the command read: null if it read none.
    pub model: Option<ModelSchema>,
    /// The command's result, as the command's own schema gives it.
    pub result: serde_json::Value,
}

/// A command refused, or one that couldn't run: `ok` is false, and `errors` gives every
/// reason, in the order found.
#[derive(JsonSchema)]
#[schemars(rename = "Refused", transform = wire_shape)]
pub struct RefusedSchema {
    #[schemars(extend("const" = false))]
    pub ok: bool,
    /// The session as the command left it: null for a command that takes no session, a
    /// session that isn't open, or a request whose session can't be read.
    pub session: Option<SessionSchema>,
    /// The model the command read: null if it read none.
    pub model: Option<ModelSchema>,
    /// The reasons, in the order found: the first 100.
    #[schemars(length(max = 100))]
    pub errors: Vec<ErrorSchema>,
    /// How many reasons there are, listed or not.
    pub total: u64,
}

/// Every command's response: one that ran, or one refused. Every field is always present,
/// null where it doesn't apply.
#[derive(JsonSchema)]
#[schemars(rename = "Response", untagged)]
pub enum ResponseSchema {
    Ran(RanSchema),
    Refused(RefusedSchema),
}

/// What makes a command's snapshot: its JSON Schemas, from its mirror structs.
pub type MakeSnapshot = fn() -> serde_json::Value;

/// The commands whose wire types are built, each by name with what makes its snapshot, in
/// the design's order (section 7): what `schema` describes besides the document, and what
/// MCP's tools are. Made from [`Command::ALL`], each command by its name, so a command can't
/// be here and not be one `Sessions::call` runs, nor the other way round.
pub const COMMANDS: [(&str, MakeSnapshot); Command::ALL.len()] = {
    let mut commands = [("", snapshot_of(Command::Version)); Command::ALL.len()];
    let mut at = 0;
    while at < commands.len() {
        let command = Command::ALL[at];
        commands[at] = (command.as_str(), snapshot_of(command));
        at += 1;
    }
    commands
};

/// What makes `command`'s snapshot: its request's and its result's mirror structs, every
/// command matched.
const fn snapshot_of(command: Command) -> MakeSnapshot {
    match command {
        Command::Version => {
            command_schema::<session::VersionRequestSchema, session::VersionResultSchema>
        }
        Command::Open => command_schema::<session::OpenRequestSchema, session::OpenResultSchema>,
        Command::Close => command_schema::<session::CloseRequestSchema, session::CloseResultSchema>,
        Command::Status => {
            command_schema::<session::StatusRequestSchema, session::StatusResultSchema>
        }
        Command::Cancel => {
            command_schema::<session::CancelRequestSchema, session::CancelResultSchema>
        }
        Command::Begin => {
            command_schema::<transaction::BeginRequestSchema, transaction::BeginResultSchema>
        }
        Command::Commit => {
            command_schema::<transaction::CommitRequestSchema, transaction::CommitResultSchema>
        }
        Command::Rollback => {
            command_schema::<transaction::RollbackRequestSchema, transaction::RollbackResultSchema>
        }
        Command::Format => {
            command_schema::<transaction::FormatRequestSchema, transaction::FormatResultSchema>
        }
        Command::AddFeature => {
            command_schema::<edits::AddFeatureRequestSchema, edits::AddFeatureResultSchema>
        }
        Command::SetField => {
            command_schema::<edits::SetFieldRequestSchema, edits::SetFieldResultSchema>
        }
        Command::RemoveFeature => {
            command_schema::<edits::RemoveFeatureRequestSchema, edits::RemoveFeatureResultSchema>
        }
        Command::ReplaceText => {
            command_schema::<edits::ReplaceTextRequestSchema, edits::ReplaceTextResultSchema>
        }
        Command::Text => command_schema::<reads::TextRequestSchema, reads::TextResultSchema>,
        Command::Outline => {
            command_schema::<reads::OutlineRequestSchema, reads::OutlineResultSchema>
        }
        Command::Check => command_schema::<reads::CheckRequestSchema, reads::CheckResultSchema>,
        Command::CacheClear => {
            command_schema::<cache::CacheClearRequestSchema, cache::CacheClearResultSchema>
        }
        Command::Elements => {
            command_schema::<elements::ElementsRequestSchema, elements::ElementsResultSchema>
        }
        Command::Query => {
            command_schema::<elements::QueryRequestSchema, elements::QueryResultSchema>
        }
        Command::Schema => {
            command_schema::<schema::SchemaRequestSchema, schema::SchemaResultSchema>
        }
        Command::MassProperties => {
            command_schema::<
                measure::MassPropertiesRequestSchema,
                measure::MassPropertiesResultSchema,
            >
        }
        Command::Distance => {
            command_schema::<measure::DistanceRequestSchema, measure::DistanceResultSchema>
        }
        Command::Angle => command_schema::<measure::AngleRequestSchema, measure::AngleResultSchema>,
        Command::Render => {
            command_schema::<render::RenderRequestSchema, render::RenderResultSchema>
        }
        Command::ReadLogs => {
            command_schema::<logs::ReadLogsRequestSchema, logs::ReadLogsResultSchema>
        }
        Command::Export => {
            command_schema::<export::ExportRequestSchema, export::ExportResultSchema>
        }
        Command::MoveFeature => {
            command_schema::<edits::MoveFeatureRequestSchema, edits::MoveFeatureResultSchema>
        }
        Command::Timeline => {
            command_schema::<reads::TimelineRequestSchema, reads::TimelineResultSchema>
        }
        Command::Sketch => {
            command_schema::<sketch::SketchRequestSchema, sketch::SketchResultSchema>
        }
        Command::SketchAdd => {
            command_schema::<
                sketch_edits::SketchAddRequestSchema,
                sketch_edits::SketchEditResultSchema,
            >
        }
        Command::SketchRemove => {
            command_schema::<
                sketch_edits::SketchRemoveRequestSchema,
                sketch_edits::SketchEditResultSchema,
            >
        }
        Command::SketchSplit => {
            command_schema::<
                sketch_edits::SketchSplitRequestSchema,
                sketch_edits::SketchSplitResultSchema,
            >
        }
        Command::SketchTrim => {
            command_schema::<
                sketch_edits::SketchTrimRequestSchema,
                sketch_edits::SketchTrimResultSchema,
            >
        }
        Command::SketchUnjoin => {
            command_schema::<
                sketch_edits::SketchUnjoinRequestSchema,
                sketch_edits::SketchUnjoinResultSchema,
            >
        }
        Command::SketchMerge => {
            command_schema::<
                sketch_edits::SketchMergeRequestSchema,
                sketch_edits::SketchMergeResultSchema,
            >
        }
    }
}

/// The JSON Schema (draft 2020-12, every object's keys sorted) a snapshot (section 8)
/// records: `envelope`, the response's; or a command's ([`command_snapshot`]). None for a
/// name with no snapshot.
pub fn snapshot_value(of: &str) -> Option<serde_json::Value> {
    if of == "envelope" {
        return Some(crate::document::schema_value_of::<ResponseSchema>());
    }
    command_snapshot(of)
}

/// The snapshot of the command `of`, one of [`COMMANDS`]: `{"request": ..., "result":
/// ...}`, two standalone root schemas. None for a name that isn't one.
pub fn command_snapshot(of: &str) -> Option<serde_json::Value> {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == of)
        .map(|(_, snapshot)| snapshot())
}

/// A command's snapshot: its request's schema and its result's, each with only the
/// definitions it uses.
fn command_schema<Request: JsonSchema, Result: JsonSchema>() -> serde_json::Value {
    serde_json::json!({
        "request": pruned(crate::document::schema_value_of::<Request>()),
        "result": pruned(crate::document::schema_value_of::<Result>()),
    })
}

/// `schema`, a root schema, with only the `$defs` it reaches, from its root and from each
/// definition reached: one a transform left unreferenced (a new feature's fingerprint's,
/// once only `null` is taken there) isn't one an agent reads.
fn pruned(mut schema: serde_json::Value) -> serde_json::Value {
    let Some(serde_json::Value::Object(mut defs)) =
        schema.as_object_mut().and_then(|root| root.remove("$defs"))
    else {
        return schema;
    };
    let mut reached = BTreeSet::new();
    let mut pending = Vec::new();
    referenced(&schema, &mut pending);
    while let Some(name) = pending.pop() {
        if let Some(def) = defs.get(&name)
            && reached.insert(name)
        {
            referenced(def, &mut pending);
        }
    }
    defs.retain(|name, _| reached.contains(name));
    if let Some(root) = schema.as_object_mut()
        && !defs.is_empty()
    {
        root.insert("$defs".to_owned(), serde_json::Value::Object(defs));
    }
    schema
}

/// The definitions `schema` refers to (`"$ref": "#/$defs/Name"`) anywhere in it, each
/// added to `found`.
fn referenced(schema: &serde_json::Value, found: &mut Vec<String>) {
    match schema {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match value.as_str().and_then(|r| r.strip_prefix("#/$defs/")) {
                    Some(name) if key == "$ref" => found.push(name.to_owned()),
                    _ => referenced(value, found),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                referenced(item, found);
            }
        }
        _ => {}
    }
}

/// The snapshot `of`'s text, as its committed file holds it: written as the document's
/// schema is, its keys sorted, pretty, with a trailing newline. None for a name with no
/// snapshot.
pub fn snapshot(of: &str) -> Option<Result<String, Error>> {
    let value = snapshot_value(of)?;
    Some(crate::document::schema_text(&value, &format!("the {of}")))
}

pub mod cache;
pub mod edits;
pub mod elements;
pub mod export;
pub mod logs;
pub mod measure;
pub mod reads;
pub mod render;
pub mod schema;
pub mod session;
pub mod sketch;
pub mod sketch_edits;
pub mod transaction;

#[cfg(test)]
mod adversarial;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;
