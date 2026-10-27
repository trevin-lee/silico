//! M4.7's wire types (docs/design/m4.1-command-api.md, section 7, `read_logs`): its
//! request, read strictly and written back as a client writes it; its result, written field
//! for field; and the mirrors its snapshot, `commands/read_logs.json`, is generated from.

use schemars::JsonSchema;

use super::{Json, RequestFields, ToWire, wire_shape, words};
use crate::command::logs::{LOG_READ_EVENTS, LogEvent, LogLevel, LogQuery, Logs};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::{Problems, SessionId};
use crate::document::FeatureId;
use crate::document::json::Node;

/// The levels, by the words a request and a result write them in, from the most severe.
fn levels() -> [(&'static str, LogLevel); 5] {
    LogLevel::ALL.map(|level| (level.word(), level))
}

/// A `read_logs` request: the session, and what to read of its logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadLogsRequest {
    pub session: SessionId,
    pub query: LogQuery,
}

impl ReadLogsRequest {
    /// The request `node` holds, or every reason its shape is refused. Its values are the
    /// command's to check (the limit's range, the cursor against the session's events).
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let level = f.word("level", &levels(), "levels");
            let feature = f.nullable_feature("feature");
            let after = f.nullable_u64("after");
            let limit = f.u32("limit");
            Some(ReadLogsRequest {
                session: session?,
                query: LogQuery {
                    level: level?,
                    feature: feature?,
                    after: after?,
                    limit: limit?,
                },
            })
        })
    }
}

impl ToWire for ReadLogsRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        let q = &self.query;
        Json::object([
            ("session", self.session.to_wire()),
            ("level", Json::str(q.level.word())),
            (
                "feature",
                Json::or_null(q.feature.as_ref(), |f| Json::str(f.as_str())),
            ),
            ("after", Json::or_null(q.after, Json::whole)),
            ("limit", Json::whole(q.limit.into())),
        ])
    }
}

impl ToWire for Logs {
    /// `read_logs`' result: the events, the cursor for the next read, and how many the
    /// buffer dropped.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("events", Json::array(self.events.iter().map(event_to_wire))),
            ("next", Json::whole(self.next)),
            ("dropped", Json::whole(self.dropped)),
        ])
    }
}

fn event_to_wire(event: &LogEvent) -> Json<'_> {
    Json::object([
        ("seq", Json::whole(event.seq)),
        ("level", Json::str(event.level.word())),
        ("at_ms", Json::whole(event.at_ms)),
        (
            "spans",
            Json::array(event.spans.iter().map(|span| Json::str(span))),
        ),
        (
            "feature",
            Json::or_null(event.feature.as_ref(), |f| Json::str(f.as_str())),
        ),
        ("message", Json::str(&event.message)),
    ])
}

/// An event's level.
pub struct LogLevelSchema;

impl JsonSchema for LogLevelSchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LogLevel".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        words(
            "An event's level, from the most severe: error, warn, info, debug, trace.",
            LogLevel::ALL.map(LogLevel::word),
        )
    }
}

/// `read_logs`' request: the session's recent log events, of a level and every level more
/// severe, of one feature or all, after a cursor.
#[derive(JsonSchema)]
#[schemars(rename = "ReadLogsRequest", transform = wire_shape)]
pub struct ReadLogsRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    /// The least severe level to read: it and every level more severe (`info` reads error,
    /// warn, and info).
    pub level: LogLevelSchema,
    /// The feature whose events to read (an event's feature, or its spans'), or null for
    /// every event.
    pub feature: Option<FeatureId>,
    /// Read the events after this one: the last read's `next`; or null, from the oldest
    /// kept. Refused if it's past the session's newest event.
    pub after: Option<u64>,
    /// How many events to read at most.
    #[schemars(range(min = 1, max = LOG_READ_EVENTS))]
    pub limit: u32,
}

/// One event a session kept. Its time and its place among events of other threads depend
/// on the machine and on scheduling: diagnostics, never results.
#[derive(JsonSchema)]
#[schemars(rename = "LogEvent", transform = wire_shape)]
pub struct LogEventSchema {
    /// Its number in the session, from 1, one more for each event the session kept.
    #[schemars(range(min = 1))]
    pub seq: u64,
    pub level: LogLevelSchema,
    /// When it happened, in milliseconds since the session opened.
    pub at_ms: u64,
    /// The spans it happened in, from the command's down: the command (with `model=published`
    /// for a read of the published model), then each span within it, its name and fields
    /// (`step feature=f0000001`).
    pub spans: Vec<String>,
    /// The feature it's of, its own or its innermost span's: null for an event of none.
    pub feature: Option<FeatureId>,
    /// Its message, then its other fields (`key=value`); cut, ending in `…`, to fit one read.
    pub message: String,
}

/// `read_logs`' result.
#[derive(JsonSchema)]
#[schemars(rename = "ReadLogsResult", transform = wire_shape)]
pub struct ReadLogsResultSchema {
    /// The events read, oldest first.
    pub events: Vec<LogEventSchema>,
    /// Where the next read goes on from, its `after`: the last event returned, if the read
    /// stopped at its limit or at 1 MiB of text; else the session's newest event.
    pub next: u64,
    /// How many events the session's buffer dropped since it opened, oldest first, to keep
    /// within its bound.
    pub dropped: u64,
}
