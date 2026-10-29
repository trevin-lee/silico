//! The core's commands, which every client (the CLI, MCP, and the app) runs, so each does
//! exactly what the others do: a session's ([`Sessions`]: a document held open between
//! commands, each session's commands run in turn on a thread of its own, each answered by a
//! [`Running`] handle; docs/design/m4.1-command-api.md, section 3), which reads and writes
//! its document's file; and every command by its name, for a transport that has a command's
//! name and its request's text ([`Sessions::call`], MCP's), built on the typed methods.
//! Beneath them, for a caller with a document or a model of its own (a
//! test, a tool on an engine it holds): a document's bytes through the gate ([`gate`]),
//! rebuilt ([`rebuild`]), checked ([`check`]), written in canonical form with its
//! fingerprints ([`format`]), and its outline ([`outline`]), each giving structured errors.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::Error;
use crate::document::{
    self, Checked, Document, Feature, FeatureId, Fingerprint, Length, NamingScheme, Operation,
};
use crate::kernel::Kernel;
use crate::recompute::{Model, Recompute, RunOutcome, Save};

mod call;
mod edit;
pub mod elements;
pub mod export;
pub mod logs;
mod lookup;
pub mod measure;
pub mod meshes;
pub mod render;
pub mod schema;
mod session;
mod session_command;
pub mod wire;

pub use call::{Answered, Call, Calling, Command};
pub use session::{
    Added, Begun, CacheCleared, Cancelled, CheckAnswer, CheckModel, Closed, Committed, FieldSet,
    MAX_PUBLISHED_READS, MAX_SESSIONS, ModelChoice, ModelRead, NOT_RELOADED, Opened, OutlineAnswer,
    Progress, Published, Read, ReadClient, Rebuild, RebuildState, Reformatted, Removed, Replaced,
    Response, Revision, RolledBack, Running, SessionId, SessionInfo, Sessions, Status, Stepped,
    Stopper, TOO_LONG, TextAnswer, TimelineAnswer, TryAgainBy, fault_suggestion,
};
pub use session_command::{ReadCommand, SessionCommand};

pub use crate::warnings::Warnings;

/// The command API's version (docs/design/m4.1-command-api.md, section 8): one more for
/// each change to a request's or result's shape that an older client couldn't read.
const API_VERSION: u32 = 1;

/// What `version` answers: the API's version, the document format's and naming scheme's,
/// and every command's limits (the design's section 6), as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub api: u32,
    pub format: u32,
    pub naming: u32,
    pub limits: Limits,
}

/// The limits `version` gives: each the constant the commands hold to, so they can't differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A document's canonical text, in bytes.
    pub document_bytes: usize,
    /// The most bytes read of a request, or of a document's file.
    pub request_bytes: usize,
    /// How deeply a request's (or a document's) values may nest.
    pub request_depth: usize,
    /// A response's text, in bytes.
    pub response_bytes: usize,
    /// The errors and warnings listed, the rest counted.
    pub listed: usize,
    /// The sessions open in one process.
    pub sessions: usize,
    /// The published reads running at once in one session.
    pub published_reads: usize,
    /// The most entries an `elements` call lists, and the most its `limit` asks for.
    pub elements: u32,
    /// The most entries a `query` returns, ties at the cut and all, and the most its
    /// `limit` asks for.
    pub query: u32,
    /// The most ranges a `query`'s `measures` holds: as many as there are measures.
    pub query_measures: usize,
    /// The elements one render highlights.
    pub highlight: usize,
    /// A render's side, in pixels, at most.
    pub render_side: u32,
    /// A render's time, in seconds, from when its scene is sent.
    pub render_seconds: u64,
    /// The most events one `read_logs` returns, and the most its `limit` asks for.
    pub log_read_events: u32,
    /// The most text one `read_logs` returns, in bytes.
    pub log_read_bytes: usize,
    /// The most events a session's log buffer keeps.
    pub log_events: usize,
    /// The most text a session's log buffer keeps, in bytes.
    pub log_bytes: usize,
    /// The least and the most tolerance a 3MF export takes.
    pub export_tolerance: [Length; 2],
    /// The most triangles one 3MF export writes, in all.
    pub export_triangles: u64,
    /// The most bytes one 3MF export's archive may be.
    pub export_bytes: u64,
    /// A kernel request's time, in seconds.
    pub kernel_seconds: u64,
    /// A sketch solve's time, in seconds.
    pub sketch_seconds: u64,
}

/// What `version` answers.
pub const VERSION: Version = Version {
    api: API_VERSION,
    format: document::FormatVersion::NUMBER,
    naming: NamingScheme::NUMBER,
    limits: Limits {
        document_bytes: document::json::MAX_DOCUMENT_LEN,
        request_bytes: document::json::MAX_INPUT_LEN,
        request_depth: document::json::MAX_DEPTH,
        response_bytes: wire::MAX_RESPONSE_LEN,
        listed: document::PROBLEMS_LISTED,
        sessions: MAX_SESSIONS,
        published_reads: MAX_PUBLISHED_READS,
        elements: elements::ENTRIES_AT_MOST,
        query: elements::ENTRIES_AT_MOST,
        query_measures: elements::MEASURES_AT_MOST,
        highlight: render::MAX_HIGHLIGHTS,
        render_side: silico_protocol::MAX_RENDER_SIDE,
        render_seconds: crate::render::RENDER_TIMEOUT.as_secs(),
        log_read_events: logs::LOG_READ_EVENTS,
        log_read_bytes: logs::LOG_READ_BYTES,
        log_events: logs::LOG_EVENTS,
        log_bytes: logs::LOG_BYTES,
        export_tolerance: export::TOLERANCES,
        export_triangles: silico_protocol::MAX_EXPORT_TRIANGLES,
        export_bytes: crate::export::MAX_ARCHIVE,
        kernel_seconds: crate::kernel::DEFAULT_TIMEOUT.as_secs(),
        sketch_seconds: crate::sketch::SKETCH_TIMEOUT.as_secs(),
    },
};

/// What was wrong with a document: every problem found, in the order found (reading, then
/// the gate, then each failed feature and each fingerprint that doesn't hold, in timeline
/// order), and how many there are, more than [`Problems::errors`] lists only when the gate
/// found too many to list, or a feature more broken references than it lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Problems {
    errors: Vec<Error>,
    total: u64,
}

impl Problems {
    fn of(errors: Vec<Error>) -> Self {
        let total = errors.len() as u64;
        Problems { errors, total }
    }

    /// `errors`, of which there are `unlisted` more than it lists (a feature's broken
    /// references past the first hundred).
    fn with_unlisted(errors: Vec<Error>, unlisted: u64) -> Self {
        let total = errors.len() as u64 + unlisted;
        Problems { errors, total }
    }

    /// The problems listed: every one; or, where there are more, the first
    /// [`document::PROBLEMS_LISTED`] the gate found, or each feature's first hundred broken
    /// references.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// How many there are, listed or not.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// None yet, to be added to as they're found ([`Problems::push`]), so every command
    /// keeps the same rule: the first [`document::PROBLEMS_LISTED`] listed, all counted.
    pub(crate) fn none() -> Self {
        Problems::of(Vec::new())
    }

    /// Whether it lists all it can: any more are only counted.
    pub(crate) fn is_full(&self) -> bool {
        self.errors.len() >= document::PROBLEMS_LISTED
    }

    /// `error` counted, and listed if there's room.
    pub(crate) fn push(&mut self, error: Error) {
        self.push_with(|| error);
    }

    /// The error `error` makes counted, and listed if there's room: made only then, so a
    /// request with a million reasons it's refused builds a hundred errors, not a million.
    pub(crate) fn push_with(&mut self, error: impl FnOnce() -> Error) {
        self.total += 1;
        if !self.is_full() {
            self.errors.push(error());
        }
    }

    /// `more` added: as many of its listed as there's room for, and every one counted,
    /// listed or not.
    pub(crate) fn absorb(&mut self, more: Problems) {
        let room = document::PROBLEMS_LISTED.saturating_sub(self.errors.len());
        self.errors.extend(more.errors.into_iter().take(room));
        self.total += more.total;
    }

    /// Nothing, if none were found; otherwise them all.
    pub(crate) fn into_result(self) -> Result<(), Problems> {
        if self.is_empty() { Ok(()) } else { Err(self) }
    }

    /// Each listed error as `f` makes it, the count kept.
    pub(crate) fn map(self, f: impl FnMut(Error) -> Error) -> Problems {
        Problems {
            errors: self.errors.into_iter().map(f).collect(),
            total: self.total,
        }
    }
}

/// The answer of a read stopped by its session closing (a read's own kernel client is
/// advanced only when its read is stopped: a cancel, whose answer the session gives,
/// `cancelled`, or its session closing, this): never `cancelled`, as of a session that
/// closed (decision 34), naming no feature, `before` what it closed before ("the render
/// finished") and `again` the command's verb ("render").
pub(crate) fn session_closed(before: &str, again: &str) -> Error {
    Error::new(
        crate::ErrorCode::InvalidInput,
        format!("the session closed before {before}"),
    )
    .with_parameter("session")
    .with_suggestion(format!("open the document again, and {again} it there"))
}

/// `e`, an error about the solid of the feature `feature`, naming the feature if it names
/// none; but the session's close ([`session_closed`]), which isn't the solid's.
pub(crate) fn of_solid(feature: &document::FeatureId, e: Error) -> Error {
    match (e.feature(), e.parameter()) {
        (Some(_), _) | (None, Some("session")) => e,
        (None, _) => e.with_feature(feature.as_str()),
    }
}

/// A read's kernel request's result: its error as the refusal, and one superseded as its
/// session closing, `closed`.
pub(crate) fn done<T>(
    outcome: crate::pool::Outcome<T>,
    closed: impl FnOnce() -> Error,
) -> Result<T, Problems> {
    match outcome {
        crate::pool::Outcome::Done(result) => result.map_err(Problems::from),
        crate::pool::Outcome::Superseded => Err(Problems::from(closed())),
    }
}

/// `bytes` read and through the gate, or what's wrong with them: the first part of a check,
/// which needs no kernel, so a client can say what's wrong with a document it can't
/// rebuild.
pub fn gate(bytes: &[u8]) -> Result<Checked, Problems> {
    match Gated::of(bytes) {
        Gated::Checked(checked) => Ok(Arc::unwrap_or_clone(checked)),
        Gated::Unread(problems) | Gated::Refused(_, problems) => Err(problems),
    }
}

/// A document's text as the reader and the gate found it: what [`gate`] gives, keeping the
/// document the gate refused, for a session that holds it to be mended.
enum Gated {
    /// It doesn't read as a document: the reader's error.
    Unread(Problems),
    /// It reads, but the gate refuses it: the document, and every problem with it.
    Refused(Document, Problems),
    /// In an `Arc`, so each rebuild of it shares it rather than copying the document.
    Checked(Arc<Checked>),
}

impl Gated {
    /// `text` read and through the gate.
    fn of(text: &[u8]) -> Self {
        let _span = tracing::info_span!("gate", bytes = text.len()).entered();
        match document::read(text) {
            Err(error) => Gated::Unread(Problems::from(error)),
            Ok(read) => match Checked::try_new(read) {
                Ok(checked) => Gated::Checked(Arc::new(checked)),
                Err((read, invalid)) => Gated::Refused(read, Problems::from(invalid)),
            },
        }
    }

    /// The document, if the text reads as one; or the reader's error.
    fn document(&self) -> Result<&Document, &Problems> {
        match self {
            Gated::Unread(problems) => Err(problems),
            Gated::Refused(document, _) => Ok(document),
            Gated::Checked(checked) => Ok(checked.document()),
        }
    }

    /// The reader's or the gate's problems: none if the gate took it.
    fn problems(&self) -> Problems {
        match self {
            Gated::Unread(problems) | Gated::Refused(_, problems) => problems.clone(),
            Gated::Checked(_) => Problems::of(Vec::new()),
        }
    }
}

impl From<Error> for Problems {
    fn from(error: Error) -> Self {
        Problems::of(vec![error])
    }
}

impl From<document::Invalid> for Problems {
    fn from(invalid: document::Invalid) -> Self {
        Problems {
            errors: invalid.errors().to_vec(),
            total: invalid.total(),
        }
    }
}

/// A gated document rebuilt on `engine`, waited for: its model; or none, if a later
/// recompute on the engine superseded it (a client that shares its engine, as the app
/// will, has the newer one's model). An error is a recompute that stopped (a bug).
pub fn rebuild<K: Kernel + 'static>(
    engine: &Recompute<K>,
    checked: Checked,
) -> Result<Option<Arc<Model>>, Error> {
    let _span = tracing::info_span!("rebuild").entered();
    Ok(match engine.start(checked).wait()? {
        RunOutcome::Published(model) => Some(model),
        RunOutcome::Superseded => None,
    })
}

/// A model's problems: each failed feature, and each fingerprint the rebuild doesn't
/// match (`Model::drift`), if any. A feature blocked by another's failure isn't a problem
/// of its own (the failure at its root is). A kernel fault the rebuild met (a worker that
/// crashed or hung, a shortage, a reply that couldn't be read) says nothing about the
/// document, so is never one of its problems: it's set aside, as a session's `check` sets
/// it aside, and the model holds it (`Model::fault`), which a session gives once, beside
/// the problems (docs/design/m4.1-command-api.md, section 6, decision 32).
///
/// For a caller with a model of its own (a test, a tool rebuilding on an engine it holds).
/// Every client of the command API checks through a session ([`Sessions::check`]); the
/// CLI's `silico check` is one.
pub fn check(model: &Model) -> Problems {
    let _span = tracing::info_span!("check").entered();
    check_except(model, &BTreeSet::new())
}

/// [`check`], but no drift of a feature in `except` (`Model::drift_except`): one a
/// session's edits reached since its last save, which its commit writes anew (decision 28).
fn check_except(model: &Model, except: &BTreeSet<FeatureId>) -> Problems {
    let mut problems = failures(model);
    let unlisted = model.unlisted_errors();
    problems.extend(own(model.drift_except(except)));
    Problems::with_unlisted(problems, unlisted)
}

/// The model's failed features, as problems of the document, a kernel fault among them
/// set aside.
fn failures(model: &Model) -> Vec<Error> {
    own(model.errors().cloned().collect())
}

/// `errors`, the document's own problems, without any that's the kernel's (a solid whose
/// measure timed out, say), which isn't: set aside, the model holding it.
fn own(errors: Vec<Error>) -> Vec<Error> {
    errors
        .into_iter()
        .filter(|e| !e.code().is_kernel_fault())
        .collect()
}

/// A document written in canonical form, its fingerprints as `engine` rebuilt it and its
/// sketches as it solved them.
#[derive(Debug, Clone, PartialEq)]
pub struct Formatted {
    /// The canonical text.
    pub text: String,
    /// The features whose fingerprints the rebuild changed (written new, or rewritten),
    /// in timeline order.
    pub refingerprinted: Vec<document::FeatureId>,
    /// The sketches whose stored values (points, circles' radii, reference dimensions'
    /// values) the solve changed, in timeline order.
    pub resolved: Vec<document::FeatureId>,
    /// What the text can't fix: the features that didn't build, or whose solids couldn't be
    /// measured, each keeping the fingerprint stored with it (but, in a session, one its
    /// edits reached, saved with none: decision 28, amended), and the drift of each feature
    /// whose record it leaves as stored. [`format`] writes every record it can, so its drift
    /// is only of measures a fingerprint can't hold.
    pub problems: Problems,
}

/// The model's document in canonical form with each feature's fingerprint as the model
/// rebuilt it and each sketch as it solved (`Model::saved`): the text a session's `format`
/// with no features named writes (what `silico fmt` runs); or, if its canonical text would
/// be too long, that problem. A kernel fault is set aside, as [`check`] sets it aside: each
/// feature it stopped keeps what's stored with it, as a failed one does, and the rest is
/// written, as the session's `format` saves around it (decision 32). For a caller with a
/// model of its own, as [`check`] is.
pub fn format(model: &Model) -> Result<Formatted, Problems> {
    let _span = tracing::info_span!("format").entered();
    formatted(model, |_| Save::Recorded, &model.drifted())
}

/// The model's document in canonical form (`Model::saved_as`), each feature written as
/// `save` says: as the model rebuilt it, with no fingerprint if it didn't build, for what a
/// session's edits reached (decision 28, amended); with its fingerprint and solve as the
/// model rebuilt it, or as stored if it didn't build; or as stored. What a session's commit
/// writes (its edits' reach as rebuilt, every other as stored, decision 28) and its
/// `format` (its edits' reach as rebuilt, and every other feature's record but those whose
/// drift it doesn't accept, decision 31), as [`format`] writes every feature's record. Its
/// problems are the failures, the solids that couldn't be measured, and of `drift` (the
/// model's, `Model::drifted`), that of every feature it leaves as stored; a kernel fault
/// set aside (a feature it stopped is saved as a failed one is). `refingerprinted` lists
/// only fingerprints written, never one left out.
fn formatted(
    model: &Model,
    save: impl Fn(&FeatureId) -> Save,
    drift: &[(FeatureId, Error)],
) -> Result<Formatted, Problems> {
    let mut problems = failures(model);
    let unlisted = model.unlisted_errors();
    let writes: BTreeSet<FeatureId> = model
        .document()
        .features
        .iter()
        .map(|feature| &feature.id)
        .filter(|id| save(id) != Save::Stored)
        .cloned()
        .collect();
    let written = model.saved_as(save);
    let pairs = || model.document().features.iter().zip(&written.features);
    let refingerprinted = pairs()
        .filter(|(before, after)| {
            after.fingerprint.is_some() && before.fingerprint != after.fingerprint
        })
        .map(|(before, _)| before.id.clone())
        .collect();
    // Bit for bit, as drift compares them: a coordinate solved to -0 from 0 is written so.
    let resolved = pairs()
        .filter(
            |(before, after)| match (&before.operation, &after.operation) {
                (document::Operation::Sketch(stored), document::Operation::Sketch(written)) => {
                    !crate::sketch::moved(stored, written).is_empty()
                }
                _ => false,
            },
        )
        .map(|(before, _)| before.id.clone())
        .collect();
    let text = match document::write(&written) {
        Ok(text) => text,
        Err(error) => {
            // What the save writes, fingerprints and solved sketches, counts as the rest
            // does (the maintainer, 2026-10-21): said when it's what takes the document past.
            let error = if document::fits(model.document()) {
                error.with_cause(
                    "the fingerprints and solved sketches a save writes take its canonical \
                     text past the limit; as it was read, it fits",
                )
            } else {
                error
            };
            return Err(Problems::of(vec![error]));
        }
    };
    // Problems the save can't fix: the failures, the solids that couldn't be measured, and
    // the drift of what it leaves as stored; drift of what it writes is what it fixes.
    problems.extend(own(model
        .drift_left(drift, &writes)
        .map(|(_, error)| error.clone())
        .collect()));
    Ok(Formatted {
        text,
        refingerprinted,
        resolved,
        problems: Problems::with_unlisted(problems, unlisted),
    })
}

/// The document's outline, for reading and for `git diff` (`silico show`): one line per
/// feature, in timeline order, saying what it is, its values as the file writes them,
/// and its fingerprint, if stored; its note, if any, on the line after; for a sketch, its
/// points, curves, and constraints after that, a line each as the file writes them, and
/// for a fillet or chamfer, its edges, a line each; and the bodies the timeline ends
/// with. Nothing is aligned in columns, so an edit to one feature, one of a sketch's
/// elements, or one edge, changes its lines alone.
pub fn outline(document: &Document) -> String {
    // Every field taken apart, so one added to a document or a feature is a compile error
    // here until the outline says it or says why not: what it leaves out, `git diff`
    // through it never shows. The format's version, the units, the naming scheme, and a
    // feature's version each have one value so far.
    let Document {
        silico: _,
        units: _,
        naming: NamingScheme,
        features,
    } = document;
    let mut out = String::new();
    for feature in features {
        let Feature {
            id,
            operation,
            version: _,
            name,
            note,
            fingerprint,
        } = feature;
        let _ = write!(
            out,
            "{} {} {}",
            id,
            quoted(name.as_str()),
            operation.type_name()
        );
        let values = values(operation);
        if !values.is_empty() {
            let _ = write!(out, " {values}");
        }
        if let Some(fingerprint) = fingerprint {
            let _ = write!(out, " | {}", measures(fingerprint));
        }
        out.push('\n');
        if let Some(note) = note {
            let _ = writeln!(out, "  note {}", quoted(note));
        }
        // A sketch's points, curves, and constraints, a line each, as the file writes them,
        // so an edit to one changes its line alone.
        if let Operation::Sketch(sketch) = operation {
            for (kind, text) in document::sketch_elements(sketch) {
                let _ = writeln!(out, "  {kind} {text}");
            }
        }
        // A fillet's or chamfer's edges, a line each, quoted (a name can be long, and in a
        // document the gate hasn't passed, anything), so an edit to one changes its line.
        for edge in operation.edges() {
            let _ = writeln!(out, "  edge {}", quoted(edge.as_str()));
        }
    }
    let bodies: Vec<&str> = bodies(document).into_iter().map(|id| id.as_str()).collect();
    let _ = writeln!(
        out,
        "bodies {}",
        if bodies.is_empty() {
            "none".to_owned()
        } else {
            bodies.join(", ")
        }
    );
    out
}

/// A feature's values, as the file names and writes them.
fn values(operation: &Operation) -> String {
    let at = |p: &document::Position| format!("at ({}, {}, {})", p[0], p[1], p[2]);
    match operation {
        Operation::Box {
            position,
            length,
            width,
            height,
        } => format!(
            "length {length}, width {width}, height {height}, {}",
            at(position)
        ),
        Operation::Cylinder {
            position,
            radius,
            height,
        } => format!("radius {radius}, height {height}, {}", at(position)),
        Operation::Sphere { position, radius } => format!("radius {radius}, {}", at(position)),
        Operation::Cone {
            position,
            radius1,
            radius2,
            height,
        } => format!(
            "radius1 {radius1}, radius2 {radius2}, height {height}, {}",
            at(position)
        ),
        Operation::Torus {
            position,
            radius1,
            radius2,
        } => format!("radius1 {radius1}, radius2 {radius2}, {}", at(position)),
        Operation::Fuse { base, tool }
        | Operation::Cut { base, tool }
        | Operation::Common { base, tool } => format!("base {base}, tool {tool}"),
        // Its elements follow, a line each.
        Operation::Sketch(sketch) => format!("plane {}", sketch.plane.as_str()),
        // Its edges follow, a line each.
        Operation::Fillet { base, radius, .. } => format!("base {base}, radius {radius}"),
        Operation::Chamfer { base, size, .. } => {
            format!("base {base}, size.distance {}", size.distance)
        }
        Operation::Pad { .. }
        | Operation::Pocket { .. }
        | Operation::Revolve { .. }
        | Operation::Groove { .. } => {
            let mut values = Vec::new();
            if let Some((_, base)) = operation.inputs().first() {
                values.push(format!("base {base}"));
            }
            if let Some(sketch) = operation.sketch() {
                values.push(format!("sketch {sketch}"));
            }
            if let Some(axis) = operation.axis() {
                values.push(format!("axis {axis}"));
            }
            if let Some(extent) = operation.extent() {
                values.push(format!("extent {}", extent.type_name()));
                for (field, value) in extent.values() {
                    values.push(match value {
                        document::ExtentValue::Length(length) => format!("extent.{field} {length}"),
                        document::ExtentValue::Angle(angle) => format!("extent.{field} {angle}"),
                        document::ExtentValue::Reversed(reversed) => {
                            format!("extent.{field} {reversed}")
                        }
                    });
                }
            }
            values.join(", ")
        }
    }
}

/// A fingerprint's measures, as the file writes them.
fn measures(f: &Fingerprint) -> String {
    let bbox = match &f.bbox {
        Some(c) => format!(
            "box ({}, {}, {}) to ({}, {}, {})",
            c[0], c[1], c[2], c[3], c[4], c[5]
        ),
        None => "no box".to_owned(),
    };
    format!(
        "volume {}, area {}, {bbox}, {} faces, {} edges, {} vertices",
        f.volume, f.area, f.faces, f.edges, f.vertices
    )
}

/// The features that make a solid and that no later feature consumes, in timeline order:
/// the solids the timeline ends with, if every feature builds.
fn bodies(document: &Document) -> Vec<&document::FeatureId> {
    let consumed: std::collections::BTreeSet<&document::FeatureId> = document
        .features
        .iter()
        .flat_map(|f| f.operation.inputs().into_iter().map(|(_, id)| id))
        .collect();
    document
        .features
        .iter()
        // A sketch makes no solid, so it's no body.
        .filter(|f| f.operation.makes_solid() && !consumed.contains(&f.id))
        .map(|f| &f.id)
        .collect()
}

/// A number's text as a whole number: digits only, with no leading zero but `0` itself, and
/// at most `u64::MAX`; none for anything else. A request's whole numbers and a session's ID
/// are read so.
fn whole_number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = text.len() > 1 && text.starts_with('0');
    if !digits || leading_zero {
        return None;
    }
    text.parse().ok()
}

/// `text` in double quotes, its quotes, backslashes, and control and invisible characters
/// escaped, so a name or note can't break the outline's lines or hide in them; and a client
/// printing other text (a file's name) can do the same.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if document::json::unseen(c) => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    fn error(n: usize) -> Error {
        Error::new(ErrorCode::InvalidInput, format!("reason {n}"))
    }

    #[test]
    fn problems_list_the_first_hundred_and_count_every_one() {
        let mut problems = Problems::none();
        assert_eq!(problems.clone().into_result(), Ok(()));
        let mut made = 0;
        for n in 0..150 {
            problems.push_with(|| {
                made += 1;
                error(n)
            });
        }
        // 150 counted, 100 listed, and only the listed made.
        assert_eq!(
            (problems.errors().len(), problems.total(), made),
            (100, 150, 100)
        );
        assert!(problems.is_full());
        assert_eq!(problems.errors()[99].message(), "reason 99");
        problems.push(error(150));
        assert_eq!(problems.total(), 151);
        assert_eq!(problems.into_result().unwrap_err().total(), 151);
    }

    #[test]
    fn problems_absorbed_keep_their_totals_and_fill_only_the_room_left() {
        // 60 found, then a feature's 100 listed of 150: 40 of those listed, 210 counted.
        let mut problems = Problems::none();
        for n in 0..60 {
            problems.push(error(n));
        }
        problems.absorb(Problems::with_unlisted((0..100).map(error).collect(), 50));
        assert_eq!((problems.errors().len(), problems.total()), (100, 210));
        assert_eq!(problems.errors()[60].message(), "reason 0");
        assert_eq!(problems.errors()[99].message(), "reason 39");
        // Absorbed into none: every one of its own.
        let mut fresh = Problems::none();
        fresh.absorb(Problems::with_unlisted(vec![error(0)], 7));
        assert_eq!((fresh.errors().len(), fresh.total()), (1, 8));
    }
}
