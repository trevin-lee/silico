//! A session's logs (M4.7, docs/design/m4.1-command-api.md, `read_logs`): the `tracing`
//! events of the session's own work, its commands and rebuilds and the kernel requests they
//! make, the workers' lines re-emitted under the calls that asked for them included, kept in
//! a bounded buffer of the session's own and read back by level, feature, and a cursor.
//!
//! The events are a subscriber's: a process keeps them only if its subscriber has this
//! module's [`layer`], which files each event under the session whose command span it falls
//! in ([`LogBuffer::attach`]: the session's buffer is kept on that span, so only the spans
//! of a session's own work lead to it, and one session's events never reach another's). An
//! event in no session's span (the process's own, a CLI's) is kept by none. The layer keeps
//! every event its subscriber gives it, so the subscriber itself must filter nothing: a
//! filter on the subscriber (`fmt().with_env_filter`) disables what it leaves out for every
//! layer, the sessions' spans among them (info), and a read is refused, as of a process
//! that keeps none. Filter the other layers each (`Layer::with_filter`), as
//! [`crate::logging::init`] filters its log to stderr.
//!
//! Logs are diagnostics (the design's section 10): their times, and the order of events on
//! different threads, depend on the machine and on scheduling, and none reaches a result, a
//! name, or a file.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::{LookupSpan, Registry, SpanData as _};

use super::SessionId;
use crate::document::FeatureId;
use crate::worker::lock;
use crate::{Error, ErrorCode};

/// The most events one read returns, and the most its `limit` asks for.
pub const LOG_READ_EVENTS: u32 = 1000;

/// The most text one read returns, in bytes (its events' messages, spans, and features):
/// a read stops before the event that would pass it, and its cursor says where.
pub const LOG_READ_BYTES: usize = 1 << 20;

/// The most events a session's buffer keeps: past it, the oldest are dropped and counted.
pub const LOG_EVENTS: usize = 10_000;

/// The most text a session's buffer keeps, in bytes, counted as a read counts it: past it,
/// the oldest events are dropped and counted.
pub const LOG_BYTES: usize = 16 << 20;

/// The most of a span's text an event carries (its name and fields), in bytes: the rest is
/// cut.
const SPAN_BYTES: usize = 4096;

/// What ends text that was cut.
const CUT: &str = "…";

/// An event's level, from the most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// Every level, from the most severe: the order a read's `level` takes them in, each
    /// and those before it.
    pub const ALL: [LogLevel; 5] = [
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Debug,
        LogLevel::Trace,
    ];

    /// The level's word, as requests and results write it.
    pub const fn word(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }

    fn of(level: Level) -> LogLevel {
        match level {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            Level::DEBUG => LogLevel::Debug,
            _ => LogLevel::Trace,
        }
    }
}

/// One event a session kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    /// Its number in the session, from 1, one more for each event the session kept.
    pub seq: u64,
    pub level: LogLevel,
    /// When it happened, in milliseconds since the session's buffer was made (as `open`
    /// started).
    pub at_ms: u64,
    /// The spans it happened in, from the command's down: the command (`check`, or `check
    /// model=published` for a read of the published model), then each span within it, its
    /// name and fields (`step feature=f0000001`).
    pub spans: Vec<String>,
    /// The feature it's of: the event's own `feature` field, or else its innermost span's
    /// that has one; none for an event of no feature.
    pub feature: Option<FeatureId>,
    /// Its message, then its other fields (`key=value`, its `feature` among them if it has
    /// one), cut to fit one read.
    pub message: String,
}

impl LogEvent {
    /// Its text's bytes, as a read and the buffer count them.
    fn bytes(&self) -> usize {
        self.message.len()
            + self.feature.as_ref().map_or(0, |f| f.as_str().len())
            + self.spans.iter().map(String::len).sum::<usize>()
    }
}

/// What a read asks for: events of `level` and every level more severe, of `feature` only
/// if one is given, after the event numbered `after` (`None` from the oldest kept), at most
/// `limit` of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogQuery {
    pub level: LogLevel,
    pub feature: Option<FeatureId>,
    pub after: Option<u64>,
    pub limit: u32,
}

/// What a read answers: the events, oldest first; where the next read goes on from (pass it
/// as its `after`); and how many events the buffer dropped since the session opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logs {
    pub events: Vec<LogEvent>,
    pub next: u64,
    pub dropped: u64,
}

/// A session's buffer of events: at most [`LOG_EVENTS`] and [`LOG_BYTES`] of text, the
/// oldest dropped and counted past either.
pub(crate) struct LogBuffer {
    made: Instant,
    /// Whether the process's subscriber keeps events for it: set once a span of the
    /// session's is attached to it ([`LogBuffer::attach`]).
    kept: AtomicBool,
    events: Mutex<Kept>,
}

#[derive(Default)]
struct Kept {
    events: VecDeque<LogEvent>,
    /// The text of `events`, in bytes.
    bytes: usize,
    /// The newest event's number: 0 before the first.
    newest: u64,
    dropped: u64,
}

impl LogBuffer {
    pub(crate) fn new() -> Arc<LogBuffer> {
        Arc::new(LogBuffer {
            made: Instant::now(),
            kept: AtomicBool::new(false),
            events: Mutex::default(),
        })
    }

    /// Files the events in `span` and every span within it under this buffer, the span
    /// written as `label` in each event's spans: a session's command span, labelled with
    /// the command's name. Does nothing if the process's subscriber hasn't [`layer`] (or
    /// isn't `tracing_subscriber`'s registry, or filtered the span out), so its events are
    /// kept by no session's buffer, and a read says so.
    pub(crate) fn attach(self: &Arc<Self>, span: &tracing::Span, label: String) {
        let attached = span.with_subscriber(|(id, dispatch)| {
            if !dispatch.is::<LogLayer>() {
                return false;
            }
            let Some(data) = dispatch
                .downcast_ref::<Registry>()
                .and_then(|registry| registry.span_data(id))
            else {
                return false;
            };
            data.extensions_mut().replace(SessionSpan {
                buffer: Arc::clone(self),
                label,
            });
            true
        });
        if attached == Some(true) {
            self.kept.store(true, Ordering::Relaxed);
        }
    }

    /// Keeps an event, numbered the next, and drops the oldest past the buffer's bound.
    fn push(
        &self,
        level: LogLevel,
        spans: Vec<String>,
        feature: Option<FeatureId>,
        message: String,
    ) {
        let mut kept = lock(&self.events);
        // Under the lock, so the times rise with the numbers.
        let at_ms = u64::try_from(self.made.elapsed().as_millis()).unwrap_or(u64::MAX);
        kept.newest += 1;
        let event = LogEvent {
            seq: kept.newest,
            level,
            at_ms,
            spans,
            feature,
            message,
        };
        kept.bytes += event.bytes();
        kept.events.push_back(event);
        while kept.events.len() > LOG_EVENTS || kept.bytes > LOG_BYTES {
            let Some(old) = kept.events.pop_front() else {
                break;
            };
            kept.bytes -= old.bytes();
            kept.dropped += 1;
        }
    }

    /// The events `query` asks for, of the session `session`, oldest first: those after
    /// its cursor, of its level or one more severe, and of its feature if it gives one, until
    /// its limit, or until the next would pass [`LOG_READ_BYTES`] (the first is returned
    /// whatever its size), the cursor then the last returned; otherwise the cursor is the
    /// newest event, every one having been read. Refused, naming the field, for a limit
    /// past what a read returns, then if the process keeps no events for the session, then
    /// for a cursor past the newest event.
    pub(crate) fn read(&self, session: SessionId, query: &LogQuery) -> Result<Logs, Error> {
        if query.limit == 0 || query.limit > LOG_READ_EVENTS {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "limit must be from 1 to {LOG_READ_EVENTS}, got {}",
                    query.limit
                ),
            )
            .with_parameter("limit")
            .with_suggestion(format!(
                "read in pages of at most {LOG_READ_EVENTS}, with after set to the last read's \
                 next"
            )));
        }
        // Before the cursor is checked: no cursor is right of a buffer kept by none.
        if !self.kept.load(Ordering::Relaxed) {
            return Err(Error::new(
                ErrorCode::Internal,
                format!(
                    "this process keeps no events of {session}: its tracing subscriber has no \
                     session log layer, or filters out the session's spans"
                ),
            )
            .with_suggestion(
                "run Silico as `silico serve`, whose subscriber keeps each session's logs \
                 (a process of its own adds silico_core::command::logs::layer to its subscriber)",
            ));
        }
        let kept = lock(&self.events);
        if let Some(after) = query.after.filter(|&after| after > kept.newest) {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "after must be at most {}, the newest event of {session}, got {after}",
                    kept.newest
                ),
            )
            .with_parameter("after")
            .with_suggestion(
                "give the last read's next, or null to read from the oldest event kept",
            ));
        }
        // The kept events are numbered on from the first, one by one.
        let first = kept.newest + 1 - kept.events.len() as u64;
        let from = query
            .after
            .map_or(0, |after| (after + 1).saturating_sub(first));
        let limit = query.limit as usize;
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut stopped = None;
        for event in kept
            .events
            .iter()
            .skip(usize::try_from(from).unwrap_or(usize::MAX))
        {
            let wanted = event.level <= query.level
                && query
                    .feature
                    .as_ref()
                    .is_none_or(|feature| event.feature.as_ref() == Some(feature));
            if !wanted {
                continue;
            }
            if events.len() == limit
                || (!events.is_empty() && bytes + event.bytes() > LOG_READ_BYTES)
            {
                stopped = events.last().map(|last: &LogEvent| last.seq);
                break;
            }
            bytes += event.bytes();
            events.push(event.clone());
        }
        Ok(Logs {
            next: stopped.unwrap_or(kept.newest),
            dropped: kept.dropped,
            events,
        })
    }
}

/// The layer that keeps each session's events in its buffer: add it to the process's
/// subscriber (`tracing_subscriber::registry().with(layer())`), and `read_logs` reads them.
/// Without it, `read_logs` is refused.
pub fn layer() -> LogLayer {
    LogLayer { _private: () }
}

/// The layer [`layer`] makes.
#[derive(Debug, Clone, Copy)]
pub struct LogLayer {
    _private: (),
}

/// A session's command span's mark: its buffer, and how its events' spans write it.
struct SessionSpan {
    buffer: Arc<LogBuffer>,
    label: String,
}

impl<S> Layer<S> for LogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = SpanFields {
            text: attrs.metadata().name().to_owned(),
            feature: None,
        };
        attrs.record(&mut fields);
        span.extensions_mut().replace(fields);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if let Some(fields) = span.extensions_mut().get_mut::<SpanFields>() {
            values.record(fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        // From the innermost span out, to the session's.
        let mut spans = Vec::new();
        let mut feature = None;
        let mut buffer = None;
        for span in scope {
            let extensions = span.extensions();
            if let Some(session) = extensions.get::<SessionSpan>() {
                spans.push(session.label.clone());
                buffer = Some(Arc::clone(&session.buffer));
                break;
            }
            if let Some(fields) = extensions.get::<SpanFields>() {
                spans.push(fields.text.clone());
                if feature.is_none() {
                    feature.clone_from(&fields.feature);
                }
            }
        }
        let Some(buffer) = buffer else {
            return;
        };
        spans.reverse();
        let mut fields = EventFields::default();
        event.record(&mut fields);
        let feature = fields.feature.or(feature);
        let mut message = fields.message;
        if !fields.rest.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&fields.rest);
        }
        let room = LOG_READ_BYTES
            .saturating_sub(spans.iter().map(String::len).sum::<usize>())
            .saturating_sub(feature.as_ref().map_or(0, |f: &FeatureId| f.as_str().len()));
        cut(&mut message, room);
        buffer.push(
            LogLevel::of(*event.metadata().level()),
            spans,
            feature,
            message,
        );
    }
}

/// A span's text, its name and fields (`step feature=f0000001`), at most [`SPAN_BYTES`],
/// and its feature, if a field named `feature` is a feature's ID.
struct SpanFields {
    text: String,
    feature: Option<FeatureId>,
}

impl SpanFields {
    fn field(&mut self, field: &Field, value: &str) {
        if field.name() == "feature"
            && let Some(feature) = FeatureId::new(value)
        {
            self.feature = Some(feature);
        }
        if self.text.len() < SPAN_BYTES {
            let _ = write!(self.text, " {}={value}", field.name());
            cut(&mut self.text, SPAN_BYTES);
        }
    }
}

impl Visit for SpanFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.field(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.field(field, &format!("{value:?}"));
    }
}

/// An event's message, its other fields (`key=value`, space-separated), and its feature, if
/// a field named `feature` is a feature's ID.
#[derive(Default)]
struct EventFields {
    message: String,
    rest: String,
    feature: Option<FeatureId>,
}

impl EventFields {
    fn field(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
            return;
        }
        if field.name() == "feature"
            && let Some(feature) = FeatureId::new(value)
        {
            self.feature = Some(feature);
        }
        // Bounded as it grows: a read holds at most this much of a message.
        if self.rest.len() < LOG_READ_BYTES {
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            let _ = write!(self.rest, "{}={value}", field.name());
        }
    }
}

impl Visit for EventFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.field(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.field(field, &format!("{value:?}"));
    }
}

/// `text` cut to at most `room` bytes, at a character's boundary, ending in [`CUT`] if it
/// was cut (and there's room for it).
fn cut(text: &mut String, room: usize) {
    if text.len() <= room {
        return;
    }
    // The mark, if there's room for it, else as much of the text as fits.
    let mark = if room >= CUT.len() { CUT } else { "" };
    let mut at = room - mark.len();
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
    text.push_str(mark);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(level: LogLevel, after: Option<u64>, limit: u32) -> LogQuery {
        LogQuery {
            level,
            feature: None,
            after,
            limit,
        }
    }

    fn kept() -> Arc<LogBuffer> {
        let buffer = LogBuffer::new();
        buffer.kept.store(true, Ordering::Relaxed);
        buffer
    }

    fn s1() -> SessionId {
        SessionId::parse("s1").unwrap()
    }

    #[test]
    fn text_is_cut_at_a_characters_boundary_and_says_so() {
        let mut text = "aé".repeat(4);
        cut(&mut text, 7);
        // 7 bytes: "aéa" (4) and the mark (3).
        assert_eq!(text, "aéa…");
        let mut short = "abc".to_owned();
        cut(&mut short, 3);
        assert_eq!(short, "abc");
        let mut tiny = "abcdef".to_owned();
        cut(&mut tiny, 2);
        assert_eq!(tiny, "ab");
    }

    #[test]
    fn a_read_of_a_buffer_never_kept_says_the_process_keeps_none() {
        let buffer = LogBuffer::new();
        let error = buffer
            .read(s1(), &query(LogLevel::Trace, None, 10))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
    }

    /// The process's subscriber, the layer alone: each test that reads it is alone in its
    /// process (nextest runs each test in its own).
    fn keep_logs() {
        use tracing_subscriber::layer::SubscriberExt as _;
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer()))
            .unwrap();
    }

    /// A buffer attached to a span made now, labelled `label`.
    fn attached(label: &str) -> (Arc<LogBuffer>, tracing::Span) {
        let buffer = LogBuffer::new();
        let span = tracing::error_span!("session_command", session = "s1");
        buffer.attach(&span, label.to_owned());
        (buffer, span)
    }

    #[test]
    fn a_flood_of_events_keeps_the_newest_ten_thousand_and_counts_the_rest_dropped() {
        keep_logs();
        let (buffer, span) = attached("check");
        span.in_scope(|| {
            for n in 0..LOG_EVENTS + 50 {
                tracing::trace!("event {n}");
            }
        });
        let logs = buffer
            .read(s1(), &query(LogLevel::Trace, None, 1000))
            .unwrap();
        assert_eq!(logs.dropped, 50);
        // The oldest kept is the 51st, numbered so: its number in the session, not the
        // buffer.
        assert_eq!(logs.events[0].seq, 51);
        assert_eq!(logs.events[0].message, "event 50");
        assert_eq!(logs.events.len(), 1000);
        assert_eq!(logs.next, 1050);
        // From before the oldest kept: from the oldest kept.
        let early = buffer
            .read(s1(), &query(LogLevel::Trace, Some(3), 1))
            .unwrap();
        assert_eq!(early.events[0].seq, 51);
        // The newest, the last.
        let last = buffer
            .read(s1(), &query(LogLevel::Trace, Some(10_049), 1000))
            .unwrap();
        assert_eq!(messages(&last), ["event 10049"]);
        assert_eq!(last.next, 10_050);
        assert_eq!(last.dropped, 50);
    }

    fn messages(logs: &Logs) -> Vec<&str> {
        logs.events.iter().map(|e| e.message.as_str()).collect()
    }

    #[test]
    fn events_past_the_buffers_bytes_drop_the_oldest_and_a_read_stops_at_a_mebibyte() {
        keep_logs();
        let (buffer, span) = attached("check");
        // Each event its message and its one span, "check": a mebibyte in all, so 16 fill
        // the buffer and each read holds one.
        let message = "m".repeat(LOG_READ_BYTES - "check".len());
        span.in_scope(|| {
            for _ in 0..20 {
                tracing::info!("{message}");
            }
        });
        let logs = buffer
            .read(s1(), &query(LogLevel::Info, None, 1000))
            .unwrap();
        assert_eq!(logs.dropped, 4);
        assert_eq!(logs.events.len(), 1);
        assert_eq!(logs.events[0].seq, 5);
        assert_eq!(logs.events[0].message.len(), message.len());
        // Stopped by its bytes: its cursor the one it returned, so the next read goes on.
        assert_eq!(logs.next, 5);
        let on = buffer
            .read(s1(), &query(LogLevel::Info, Some(5), 1000))
            .unwrap();
        assert_eq!(on.events[0].seq, 6);
        assert_eq!(on.next, 6);
        // One a byte too long is cut to fit, ending in the mark.
        let (cut_buffer, cut_span) = attached("check");
        let longer = "é".repeat(LOG_READ_BYTES);
        cut_span.in_scope(|| tracing::warn!("{longer}"));
        let cut = cut_buffer
            .read(s1(), &query(LogLevel::Warn, None, 1))
            .unwrap();
        let event = &cut.events[0];
        assert!(event.message.ends_with(CUT), "{}", &event.message[..8]);
        assert!(event.bytes() <= LOG_READ_BYTES);
        // Cut at a character's boundary, as close to the bound as one allows: the mark and
        // the span within a character (2 bytes) of it.
        assert!(event.bytes() > LOG_READ_BYTES - 2, "{}", event.bytes());
    }

    #[test]
    fn an_events_feature_is_its_own_else_its_innermost_spans_and_its_fields_follow_its_message() {
        keep_logs();
        let (buffer, span) = attached("check model=published");
        span.in_scope(|| {
            let step = tracing::debug_span!("step", feature = "f0000001", job = "make");
            step.in_scope(|| {
                let inner = tracing::trace_span!("inner", n = 3, ok = true, what = ?"x");
                inner.in_scope(|| {
                    tracing::info!("of the step's");
                    tracing::info!(feature = "f0000002", count = 2, "its own");
                    // Not an ID: the span's, and the field kept as written.
                    tracing::info!(feature = "F0000002", "not one");
                });
            });
            tracing::error!("of none");
        });
        let logs = buffer
            .read(s1(), &query(LogLevel::Info, None, 1000))
            .unwrap();
        let read: Vec<(Option<&str>, &str, Vec<&str>)> = logs
            .events
            .iter()
            .map(|e| {
                (
                    e.feature.as_ref().map(FeatureId::as_str),
                    e.message.as_str(),
                    e.spans.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        let inner = vec![
            "check model=published",
            "step feature=f0000001 job=make",
            "inner n=3 ok=true what=\"x\"",
        ];
        assert_eq!(
            read,
            [
                (Some("f0000001"), "of the step's", inner.clone()),
                (
                    Some("f0000002"),
                    "its own feature=f0000002 count=2",
                    inner.clone()
                ),
                (Some("f0000001"), "not one feature=F0000002", inner),
                (None, "of none", vec!["check model=published"]),
            ]
        );
        // By level: error alone is the last.
        let errors = buffer
            .read(s1(), &query(LogLevel::Error, None, 1000))
            .unwrap();
        assert_eq!(messages(&errors), ["of none"]);
        // By feature: an event's own, or its span's.
        let of = |feature: &str| {
            let query = LogQuery {
                feature: FeatureId::new(feature),
                ..query(LogLevel::Trace, None, 1000)
            };
            messages(&buffer.read(s1(), &query).unwrap())
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            of("f0000001"),
            ["of the step's", "not one feature=F0000002"]
        );
        assert_eq!(of("f0000002"), ["its own feature=f0000002 count=2"]);
    }

    #[test]
    fn an_event_in_no_sessions_span_is_kept_by_none_and_one_in_a_span_within_it_is_kept() {
        keep_logs();
        let (buffer, span) = attached("check");
        tracing::info!("outside");
        let other = tracing::info_span!("other");
        other.in_scope(|| tracing::info!("in another span"));
        span.in_scope(|| tracing::info!("inside"));
        // A span within the session's, made on another thread, as a recompute's is.
        let inner = span.in_scope(|| tracing::info_span!("recompute"));
        std::thread::spawn(move || inner.in_scope(|| tracing::info!("on another thread")))
            .join()
            .unwrap();
        let logs = buffer
            .read(s1(), &query(LogLevel::Trace, None, 1000))
            .unwrap();
        assert_eq!(messages(&logs), ["inside", "on another thread"]);
        assert_eq!(logs.events[1].spans, ["check", "recompute"]);
    }

    #[test]
    fn each_event_is_numbered_on_and_the_oldest_dropped_past_the_count() {
        let buffer = kept();
        for n in 0..LOG_EVENTS + 3 {
            buffer.push(LogLevel::Info, vec![], None, format!("{n}"));
        }
        let logs = buffer.read(s1(), &query(LogLevel::Info, None, 2)).unwrap();
        assert_eq!(logs.dropped, 3);
        let seqs: Vec<u64> = logs.events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [4, 5]);
        assert_eq!(logs.events[0].message, "3");
        assert_eq!(logs.next, 5);
    }
}
