//! One open document (docs/design/m5.0-app.md, section 2, *What a tab holds*): its session,
//! the newest answers of the commands it reads each frame or on a change, the commands it
//! has under way, and its edits, at most one in flight (section 3, *Bursts of edits*).
//! Every command is started here and polled with `try_take`; nothing here waits.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::export::{Export, Exported};
use silico_core::command::wire::Named;
use silico_core::command::{
    Added, Begun, Cancellable, CheckAnswer, CheckModel, Closed, Committed, DiffAnswer, ExtendBy,
    FieldSet, ModelChoice, Moved, NewMembers, Problems, RebuildState, Removed, Replaced, Response,
    Revision, RolledBack, Running, SessionId, SessionInfo, Sessions, SketchCut, SketchEdited,
    SketchJoined, Status, StepState, Stepped, Suppressed, TextAnswer, TimelineAnswer,
};
use silico_core::disk::Disk;
use silico_core::document::json::Node;
use silico_core::document::{ElementId, FeatureId, Reference};
use silico_core::extend::End;
use silico_core::kernel::Clients;
use silico_core::sketch_tools::{AddToken, OfferChoice, OfferRequest};
use silico_core::timeline::{Entry, Health};

use crate::changes::{self, Changes};
use crate::dialog::{Dialog, Kind, Standing, feature_node, quoted};
use crate::sketch::dimension::{Ask, ModifyAction};
use crate::sketch::drag::{Drag, News};
use crate::sketch::screen::Screen;
use crate::sketch::tools::{Choice, Request};
use crate::sketch::{self, Deletion, FrameInput, Prefixes, SketchMode};

/// What's said when an over-defining dimension's choice is refused as a `conflict`: the
/// session moved on before it was sent, so it was overtaken, and isn't sent again (SD7: its
/// requests are made against the revision its add or edit made).
pub const OVERTAKEN: &str = "The dimension's choice wasn't applied: the sketch changed after it was placed (another edit came first), so it isn't sent again";

/// What a feature's mark says: its first problem's (or warning's) message, its cause and
/// the elements it names, if it has them, and how many elements are at fault in all (more
/// than it names where there are too many to list, ARCHITECTURE.md's invariant 9), and its
/// suggestion, from a `check` of the model the timeline's marks came from; whether it's a
/// warning's; and the core's error or warning itself, whole, if it's one (a refusal said
/// in words alone isn't): what its card reads its code, feature, parameter, candidates,
/// and their ranking from, and copies as the JSON an agent gets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Said {
    pub warning: bool,
    pub message: String,
    pub cause: Option<String>,
    pub elements: Vec<String>,
    pub elements_total: u64,
    pub suggestion: Option<String>,
    pub source: Option<Source>,
}

/// How many errors and warnings a `check` counted past those it listed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unlisted {
    pub errors: u64,
    pub warnings: u64,
}

/// What the core said, kept whole: an error, or a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Error(silico_core::Error),
    Warning(silico_core::Warning),
}

impl Source {
    /// Every field the error contract writes, borrowed.
    fn said(&self) -> silico_core::Said<'_> {
        match self {
            Source::Error(error) => error.said(),
            Source::Warning(warning) => warning.said(),
        }
    }
}

impl Said {
    /// What `error` says.
    pub fn of(error: &silico_core::Error) -> Self {
        Said {
            warning: false,
            message: error.message().to_owned(),
            cause: error.cause().map(str::to_owned),
            elements: error.elements().to_vec(),
            elements_total: error.elements_total(),
            suggestion: error.suggestion().map(str::to_owned),
            source: Some(Source::Error(error.clone())),
        }
    }

    /// What `warning` says.
    pub fn of_warning(warning: &silico_core::Warning) -> Self {
        Said {
            warning: true,
            message: warning.message().to_owned(),
            cause: warning.cause().map(str::to_owned),
            elements: warning.elements().to_vec(),
            elements_total: warning.elements_total(),
            suggestion: warning.suggestion().map(str::to_owned),
            source: Some(Source::Warning(warning.clone())),
        }
    }

    /// Its code, as an agent matches on it (`reference_broken`), if the core said it.
    pub fn code(&self) -> Option<&'static str> {
        self.source.as_ref().map(|source| source.said().code)
    }

    /// The feature it's about, by its ID, if the core named one.
    pub fn feature(&self) -> Option<&str> {
        self.source
            .as_ref()
            .and_then(|source| source.said().feature)
    }

    /// The field at fault, if the core named one.
    pub fn parameter(&self) -> Option<&str> {
        self.source
            .as_ref()
            .and_then(|source| source.said().parameter)
    }

    /// The names it offers in place of one broken, closest first.
    pub fn candidates(&self) -> &[String] {
        self.source
            .as_ref()
            .map_or(&[], |source| source.said().candidates)
    }

    /// Why each of its [`candidates`](Self::candidates) is where it is, as the core ranked
    /// them: one for each, or none where they follow another rule.
    pub fn ranking(&self) -> &[silico_core::Rank] {
        self.source
            .as_ref()
            .map_or(&[], |source| source.said().ranking)
    }

    /// The core's error or warning as the command API writes it, byte for byte: the error
    /// contract's shape (`tests/contract/error-full.json`), on one line, as an agent gets it
    /// over MCP. None for a refusal said in words alone, or one too long to write.
    pub fn json(&self) -> Option<String> {
        use silico_core::command::wire::{self, Layout, ToWire};
        let value = match self.source.as_ref()? {
            Source::Error(error) => error.to_wire(),
            Source::Warning(warning) => warning.to_wire(),
        };
        wire::write(&value, Layout::OneLine).ok()
    }

    /// Its elements as the card lists them: each it names, and, where it names fewer than
    /// are at fault, how many in all ("Edge1, Edge2 (the first 2 of 150)"); none if it
    /// names none.
    pub fn elements_line(&self) -> Option<String> {
        if self.elements.is_empty() {
            return None;
        }
        let listed = self.elements.join(", ");
        let named = self.elements.len() as u64;
        Some(if self.elements_total > named {
            format!(
                "Elements: {listed} (the first {named} of {})",
                self.elements_total
            )
        } else {
            format!("Elements: {listed}")
        })
    }
}

/// An edit of one field of one feature (`set_field`), as the person made it.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub feature: FeatureId,
    pub field: String,
    pub value: Node,
}

/// What a tab asks of its session, in the order the person asked (section 3, *Bursts of
/// edits*; #298's review): its edits, its saves, its reverts, and its undos and redos, one
/// at a time,
/// each sent once what's before it has answered, against the revision that gave. A newer
/// edit of a field replaces one waiting for it only if nothing else waits between them, so
/// an undo or a save never moves past an edit made after it.
#[derive(Debug, Clone, PartialEq)]
enum Queued {
    Edit(Edit),
    Save {
        then_close: bool,
    },
    /// Save As (`commit` with a path): to `path`, replacing a file there if `replace`; then,
    /// if `then_close`, close (an untitled part's Save, asked by Close or Quit).
    SaveAs {
        path: String,
        replace: bool,
        then_close: bool,
    },
    Revert {
        then_close: bool,
    },
    /// Reload's Rebuild everything (Trevin's ask of 2026-11-09), after the revert it asked,
    /// if any: its `check` reads the file first ([`Tab::reload`]). In the queue, so an edit
    /// asked after it is sent after it, and a later revert drops it.
    Reload,
    /// An undo or a redo; one asked in sketch mode carries the state it goes no further back
    /// than (`floor`), and takes one step, never a dialog's group.
    Step {
        undo: bool,
        floor: Option<Revision>,
        sketch: Option<u64>,
    },
    /// A feature dragged along the timeline (`move_feature`): `feature`, before `before`, or
    /// to the end for none.
    Move {
        feature: FeatureId,
        before: Option<FeatureId>,
    },
    /// A feature suppressed, or unsuppressed for `suppressed` false (`suppress_feature`),
    /// from the timeline's or the browser's menu.
    Suppress {
        feature: FeatureId,
        suppressed: bool,
    },
    /// A feature deleted (`remove_feature`), from a menu or by Del: what consumed it
    /// rewired to its base.
    Remove {
        feature: FeatureId,
    },
    /// A dialog's new feature (`add_feature`), of `kind`, of the dialog numbered `serial`:
    /// the members its dialog gave ([`Dialog::add`]), and, for a sweep, the body it builds
    /// on, read from the document when the add comes up ([`Body`]).
    Add {
        serial: u64,
        kind: Kind,
        members: Vec<String>,
        body: Body,
    },
    /// A dialog (or sketch mode) opened, numbered `serial`: the revision it opens at is the
    /// one everything asked before it comes to, which its Cancel undoes back to and its OK
    /// groups from.
    Opened {
        serial: u64,
    },
    /// Create Sketch's new sketch (`add_feature`), for sketch mode numbered `serial`.
    AddSketch {
        serial: u64,
        feature: Node,
    },
    /// Delete in sketch mode: `members` of the sketch `sketch` removed (`sketch_remove`).
    SketchRemove {
        sketch: FeatureId,
        members: Vec<Reference>,
    },
    /// Delete of a split point in sketch mode: the pieces of the sketch `sketch` that share
    /// `point` joined (`sketch_join`).
    SketchJoin {
        sketch: FeatureId,
        point: ElementId,
    },
    /// A sketch tool's add (`sketch_add`), of sketch mode numbered `serial`, in the sketch
    /// `sketch`: its members, and the token its answer is given back by.
    SketchAdd {
        serial: u64,
        sketch: FeatureId,
        token: AddToken,
        members: NewMembers,
    },
    /// Trim's `sketch_trim` of `curve` at `pick`.
    SketchTrim {
        sketch: FeatureId,
        curve: ElementId,
        pick: [f64; 2],
    },
    /// Split's `sketch_split` of `curve` at `at`.
    SketchSplit {
        sketch: FeatureId,
        curve: ElementId,
        at: Vec<[f64; 2]>,
    },
    /// Trim's extend (`sketch_extend`) of `curve`'s `end` `by`.
    SketchExtend {
        sketch: FeatureId,
        curve: ElementId,
        end: End,
        by: ExtendBy,
    },
    /// A placed dimension's value, of sketch mode numbered `serial`: `set_field` of the
    /// sketch's `constraints.ID.value` to `value`, as the Modify box's text read.
    DimensionValue {
        serial: u64,
        sketch: FeatureId,
        constraint: ElementId,
        value: Node,
    },
    /// An over-defining dimension's choice (SD7), of sketch mode numbered `serial`: its
    /// request, sent against the revision it names, never the tab's.
    Offer {
        serial: u64,
        request: OfferRequest,
    },
    /// Edit Feature's read of the feature `feature`, of `kind`, for the dialog numbered
    /// `serial`: sent once everything asked before it has answered, so the values it loads
    /// are those the edits before it left.
    Load {
        serial: u64,
        kind: Kind,
        feature: FeatureId,
    },
    /// A dialog closed by OK (its edits one step for undo from then) or by Cancel (undone
    /// back to the revision it opened at, `undo` with `to`, so edits made before it stay; or,
    /// if its own edits began the transaction, rolled back, there being nothing before it to
    /// keep).
    /// Sketch mode's Finish or Cancel (`sketch`) does the same, and a Finish fences Redo off
    /// at the state it ended at, so what sketch mode undid isn't redone past its edits.
    Closed {
        ok: bool,
        sketch: bool,
    },
    /// Restore (Fable's decision of 2026-11-04, section 2): the text of `revision`, the one
    /// a reload replaced, read (`text` with `revision`), then put back as an edit.
    Restore {
        revision: Revision,
    },
    /// Restore's text read, put back: `begin` if no transaction is open, then `replace_text`
    /// of it, an edit like any other.
    Replace {
        revision: Revision,
        text: String,
    },
}

/// A new sweep's body: not yet read, or read from the document at a revision (its tip,
/// [`tip_of`], none if it has none), which the add is sent with only if the session is still
/// at that revision, and read again otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    Unread,
    Read(Option<FeatureId>, Revision),
}

/// What changes the document as it's sent (an edit, an add): the revision it's sent
/// against, and the state the session is at then ([`Tab::state_of`]). Once it's applied (it
/// answers, and the session's revision moved past the one it was sent against), the
/// dialogs' groups past that state are let go: the redo branch the change dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sending {
    against: Revision,
    state: Revision,
}

/// How many of the commands it sent a tab keeps in its record ([`Tab::sent`]): the newest.
pub const SENT_KEPT: usize = 256;

/// Why a boolean's command is off in a part of fewer than two bodies ([`Tab::why_closed`]).
pub const NEEDS_TWO_BODIES: &str = "Needs two bodies";

/// Why nothing of a document the gate (or the reader) refused changes but its text: a
/// feature's menu's items and the toolbar's dialogs say it.
pub const UNMENDED: &str =
    "The document has problems of its own: no feature changes until they're mended";

/// A command a tab sent that changes its transaction, as it sent it: what a test reads
/// as the call record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    Begin,
    Add,
    Edit(String),
    /// `move_feature`, of the feature, before the feature (none: to the end).
    Move(FeatureId, Option<FeatureId>),
    /// `suppress_feature`, of the feature, suppressed or (false) unsuppressed.
    Suppress(FeatureId, bool),
    /// `remove_feature`, of the feature.
    Remove(FeatureId),
    /// `replace_text`: Restore's edit.
    Replace,
    Undo(Option<Revision>),
    Redo(Option<Revision>),
    Commit,
    Rollback,
    /// A sketch edit, by its command's name (`sketch_remove`, `sketch_join`, `sketch_add`,
    /// `sketch_trim`, `sketch_split`, `sketch_extend`), or a drag's (`drag_begin`,
    /// `drag_end`).
    SketchEdit(&'static str),
}

/// A dialog's edits, OK'd: one step for undo and redo (section 9), from the state it opened
/// at to the state it was OK'd at, each named by the first revision the session was at it
/// ([`Tab::state_of`]): an undo at its end undoes to its start, and a redo at its start
/// redoes to its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Group {
    start: Revision,
    end: Revision,
}

/// An edit in flight: its feature and field, and its value, as it was sent, and its answer
/// to come.
struct Flying {
    field: (FeatureId, String),
    value: Node,
    sending: Sending,
    running: Running<FieldSet>,
}

/// A tab's edits and what waits with them: at most one edit in flight, the rest queued
/// ([`Queued`]).
#[derive(Default)]
pub struct Edits {
    /// The edit in flight.
    in_flight: Option<Flying>,
    queue: std::collections::VecDeque<Queued>,
    /// How many edits it has sent: what a test counts.
    pub sent: u64,
}

impl Edits {
    /// Whether an edit of `feature` is in flight or queued.
    fn pending_for(&self, feature: &FeatureId) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|flying| &flying.field.0 == feature)
            || self
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Edit(edit) if &edit.feature == feature))
    }

    /// Whether no edit is in flight or queued.
    pub fn idle(&self) -> bool {
        self.in_flight.is_none()
            && !self
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Edit(_)))
    }

    /// `edit` made: in place of one of the same field that waits last, with only edits
    /// after it, or at the end.
    fn push(&mut self, edit: Edit) {
        for queued in self.queue.iter_mut().rev() {
            match queued {
                Queued::Edit(waiting)
                    if waiting.feature == edit.feature && waiting.field == edit.field =>
                {
                    *waiting = edit;
                    return;
                }
                Queued::Edit(_) => {}
                Queued::Save { .. }
                | Queued::SaveAs { .. }
                | Queued::Revert { .. }
                | Queued::Reload
                | Queued::Step { .. }
                | Queued::Move { .. }
                | Queued::Suppress { .. }
                | Queued::Remove { .. }
                | Queued::Add { .. }
                | Queued::Opened { .. }
                | Queued::AddSketch { .. }
                | Queued::SketchRemove { .. }
                | Queued::SketchJoin { .. }
                | Queued::SketchAdd { .. }
                | Queued::SketchTrim { .. }
                | Queued::SketchSplit { .. }
                | Queued::SketchExtend { .. }
                | Queued::DimensionValue { .. }
                | Queued::Offer { .. }
                | Queued::Load { .. }
                | Queued::Closed { .. }
                | Queued::Restore { .. }
                | Queued::Replace { .. } => break,
            }
        }
        self.queue.push_back(Queued::Edit(edit));
    }
}

/// A command a tab has under way, besides its reads and its edits.
enum Under {
    Begin(Running<Begun>),
    Commit {
        running: Running<Committed>,
        then_close: bool,
    },
    Rollback {
        running: Running<RolledBack>,
        then_close: bool,
    },
    /// An undo or a redo, the sketch mode it was asked in, if it was, and whether it's an
    /// over-defining dimension's Cancel (SD7), whose `conflict` is said as overtaken.
    Step {
        running: Running<Stepped>,
        undo: bool,
        sketch: Option<u64>,
        offered: bool,
    },
    /// A move along the timeline, as it was sent.
    Move {
        sending: Sending,
        running: Running<Moved>,
    },
    /// A suppression or an unsuppression, as it was sent.
    Suppress {
        sending: Sending,
        running: Running<Suppressed>,
    },
    /// A deletion of `feature`, as it was sent.
    Remove {
        feature: FeatureId,
        sending: Sending,
        running: Running<Removed>,
    },
    /// A Save As, the path it was given, which the tab asks to replace if a file is
    /// there, whether the tab began the transaction it commits (rolled back if it's
    /// refused, so no empty transaction stays open), and whether it closes the tab once
    /// it has saved.
    SaveAs {
        running: Running<Committed>,
        path: String,
        began: bool,
        then_close: bool,
    },
    Close(Running<Closed>),
    /// A dialog's add, of the dialog numbered `serial`, as it was sent.
    Add {
        serial: u64,
        sending: Sending,
        running: Running<Added>,
    },
    /// The document's features read for a new sweep's body, at the revision its add is
    /// sent against: a `timeline` of the published model, which answers at once beside the
    /// queue (a preview's read of the current model may wait on its rebuild) and lists the
    /// session's document as it is. Its add is sent once it answers.
    Base {
        serial: u64,
        kind: Kind,
        members: Vec<String>,
        running: Running<TimelineAnswer>,
    },
    /// Create Sketch's add, for sketch mode numbered `serial`, as it was sent.
    AddSketch {
        serial: u64,
        sending: Sending,
        running: Running<Added>,
    },
    /// Sketch mode's Delete, a removal, as it was sent.
    SketchEdit {
        sending: Sending,
        running: Running<SketchEdited>,
    },
    /// Sketch mode's Delete of a split point, a join, as it was sent.
    SketchJoin {
        sending: Sending,
        running: Running<SketchJoined>,
    },
    /// A sketch tool's add, of sketch mode numbered `serial`, as it was sent, with the token
    /// its answer is given back by.
    SketchAdd {
        serial: u64,
        token: AddToken,
        sending: Sending,
        running: Running<SketchEdited>,
    },
    /// Trim's or split's edit, as it was sent.
    SketchCut {
        sending: Sending,
        running: Running<SketchCut>,
    },
    /// A placed dimension's value, of sketch mode numbered `serial`, as it was sent.
    DimensionValue {
        serial: u64,
        sending: Sending,
        running: Running<FieldSet>,
    },
    /// An over-defining dimension's Driven (SD7), as it was sent.
    OfferField {
        sending: Sending,
        running: Running<FieldSet>,
    },
    /// An existing feature's text, read for its dialog (Edit Feature), of `kind`, numbered
    /// `serial`.
    Load {
        serial: u64,
        kind: Kind,
        feature: FeatureId,
        running: Running<TextAnswer>,
    },
    /// Restore's read of the text of `revision`, sent at the tab's revision `sent_at`.
    Restore {
        revision: Revision,
        sent_at: Revision,
        running: Running<TextAnswer>,
    },
    /// Restore's edit, `replace_text` of `revision`'s text, as it was sent, and whether its
    /// `begin` was sent for it.
    Replace {
        revision: Revision,
        began: bool,
        sending: Sending,
        running: Running<Replaced>,
    },
}

/// The choice offered for a file changed on disk while edits are held (section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskChoice {
    /// None offered: the file is as the session holds it, or no edits are held.
    None,
    /// Offered: reload (dropping the edits) or keep them.
    Offered(Disk),
    /// Kept: not offered again until the file is as the session holds it once more.
    Kept,
}

/// A reload of the session's own (section 4): the revision the tab had, and the one the
/// file read gave; what changed between them, as `diff` answers (DESIGN.md, 6c); and
/// Restore's refusal, said, if it was refused (the revision no longer held, after another
/// reload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloaded {
    pub from: Revision,
    pub to: Revision,
    pub changes: Changes,
    pub refused: Option<String>,
    /// The earlier reloads this one's banner replaced before their changes were listed
    /// (their `diff` still asked, or refused, the file having changed again before it ran),
    /// each from and to: their changes are never listed, nor their features marked.
    pub unlisted: Vec<(Revision, Revision)>,
    /// Whether the session still holds `from`'s text, as far as the tab knows: not once
    /// the reload's `diff` was refused for not holding it (the file changed again, and a
    /// reload queued ahead of the diff, the disk watch's own or another client's read,
    /// replaced it), when Restore, which reads it, can only be refused too.
    pub from_held: bool,
}

/// Show changes' two lists (DESIGN.md, 6d), each from its own `diff`, answered or refused
/// on its own: "Their change (on disk)", from the transaction's base to the file; and "Your
/// unsaved edits", from the base to the document then.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeldChanges {
    pub theirs: Changes,
    pub mine: Changes,
}

/// What `read`, a diff asked for a list, answered, if it has: its changes listed, or its
/// refusal said; `read` let go once it answers.
fn took(read: &mut Option<Running<DiffAnswer>>) -> Option<Changes> {
    let running = read.take()?;
    match running.try_take() {
        Ok(response) => Some(match response.result {
            Ok(answer) => Changes::Listed(changes::listed(&answer.changes)),
            Err(problems) => Changes::refused(&problems),
        }),
        Err(running) => {
            *read = Some(running);
            None
        }
    }
}

/// A row of the browser (section 8, *The browser*): a feature, by its ID and name, with the
/// health it's marked with and the feature whose mark it is (a body's, the worst of its
/// chain's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: FeatureId,
    pub name: String,
    pub health: Health,
    pub marked_by: Option<FeatureId>,
}

/// What a browser row's eye hides: one of the origin's rows, by its name, or a body or a
/// sketch, by its feature's ID (so a rename keeps it hidden, and two features of one name
/// are hidden apart).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hideable {
    Origin(&'static str),
    Feature(FeatureId),
}

/// The browser's rows, worked out once for each timeline read, never each frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Browser {
    pub bodies: Vec<Row>,
    pub sketches: Vec<Row>,
}

/// One of a tab's reads of the model that the stop cancels ([`Tab::halt`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnRead {
    Timeline,
    Check,
}

/// One open document.
pub struct Tab {
    pub id: SessionId,
    /// The path, as it was opened or as its last Save As gave it: none while it's an
    /// untitled part, which has no file until its first save.
    pub path: Option<String>,
    /// The name it's shown by while untitled ("Untitled", "Untitled 2", ...): empty for a
    /// tab opened on a file.
    untitled: String,
    /// The session's revision and whether a transaction is open, as last answered: never
    /// moved back by an answer that comes after a newer one.
    pub revision: Revision,
    pub transaction: bool,
    /// The newest `status`, and the revision it was of.
    pub status: Option<Status>,
    status_revision: Revision,
    /// The last rebuild `status` showed whole (done, every step of its plan settled and no
    /// feature left queued or running): its revision, and the core's time for it, in whole
    /// milliseconds (`elapsed_ms`). What the bar and a dialog's preview say of the
    /// revision's rebuild ([`Tab::rebuilt_ms`]).
    rebuilt: Option<(Revision, u64)>,
    /// The revision of the newest model published, if there's one.
    pub published: Option<Revision>,
    /// The newest model published, as last seen (held weakly: only to tell it from
    /// another, which a fresh check publishes at the same revision), and one more each time
    /// it was another: what the viewport tells a model its meshes are stale by.
    published_model: Option<std::sync::Weak<silico_core::recompute::Model>>,
    pub published_models: u64,
    /// The newest `timeline`, and the browser's rows worked out from it.
    pub timeline: Option<TimelineAnswer>,
    pub browser: Browser,
    timeline_read: Option<Running<TimelineAnswer>>,
    /// Whether the timeline read out was cancelled by what the tab sent since
    /// ([`supersede_preview`](Self::supersede_preview)), until it answers: the rebuild it
    /// waited on, if it did, is stopped by that cancel, the tab's own, never by a stop.
    timeline_superseded: bool,
    timeline_asked: Option<(Revision, Option<Revision>)>,
    /// A published timeline was refused, nothing being published: the next is read of the
    /// current model, which waits for its rebuild (off the UI thread).
    nothing_published: bool,
    /// Whether the timeline read out is Rebuild's, until it answers.
    rebuild_out: bool,
    /// What `check` says of each feature (read when the person opens a mark, section 3),
    /// and of the document (the gate's problems).
    pub said: BTreeMap<String, Said>,
    pub document_problems: Vec<Said>,
    /// Every problem and warning the last `check` gave, in the document's order (the
    /// document's own first, then each feature's in the timeline's order, its errors before
    /// its warnings), and the timeline it answers (its revision and model): What's Wrong's
    /// list.
    listed: Vec<Said>,
    /// How many errors and warnings that `check` counted past those it listed.
    unlisted: Unlisted,
    listed_for: Option<(Revision, Option<Revision>)>,
    /// What's Wrong is open over the window (docs/ux/solidworks.md, §5, *Errors and
    /// warnings*): while it is, the tab reads a `check` of its model whenever the timeline
    /// marks anything, to list every problem.
    pub whats_wrong: bool,
    /// The feature the timeline is to scroll to, once (an error card's Show in timeline).
    timeline_scroll: Option<FeatureId>,
    /// A `check` read, and whether it's of a document the gate refused (whose problems are
    /// all the document's).
    check_read: Option<(bool, Running<CheckAnswer>)>,
    /// Whether the check read out was cancelled by what the tab sent since
    /// ([`supersede_preview`](Self::supersede_preview)), until it answers: as
    /// `timeline_superseded` is of the timeline read, never counted as the stop's.
    check_superseded: bool,
    /// Whether the `check` read out was sent before the timeline read out, both being out:
    /// the session runs its queued commands in the order sent (a read of the published
    /// model runs on a thread of its own, waiting on no rebuild), so the later one runs
    /// after the other and is the one cancelled first ([`cancel_own_reads`]).
    ///
    /// [`cancel_own_reads`]: Self::cancel_own_reads
    check_first: bool,
    /// Whether the check read out is Rebuild everything's (`check` with `fresh`), until it
    /// answers: its answer has the timeline read again, the model it published being the
    /// revision's from then on.
    fresh_out: bool,
    checked: Option<(Revision, Option<Revision>)>,
    pub edits: Edits,
    under: Vec<Under>,
    /// How many saves it has sent: what a test counts.
    pub saves: u64,
    /// How many Rebuilds it has sent: what a test counts.
    pub rebuilds: u64,
    /// How many Rebuild everythings it has sent: what a test counts.
    pub rebuilds_all: u64,
    /// One more each time the document moved other than by the person's typing, with none
    /// of it waiting to be applied: an undo, a redo, or a revert done, or the session's own
    /// reload of its file (a revision the tab didn't ask for, with nothing of its own under
    /// way). The property panel's field then shows the document's value again. An edit
    /// refused keeps what was typed, a conflict's too: the next edit sends it again against
    /// the reloaded revision.
    pub resets: u64,
    /// A `begin` sent and not yet answered: the next edit begins none.
    beginning: bool,
    pub disk_choice: DiskChoice,
    /// The session's last reload of its file, changed on disk while nothing was held (a
    /// revision the tab didn't ask for), until the person dismisses its banner (DESIGN.md,
    /// 6c).
    pub reloaded: Option<Reloaded>,
    /// The person reverted while the file had changed on disk (Reload, or Revert after
    /// keeping their edits): the session's reload that follows is theirs, not another
    /// program's, so it shows no [`reloaded`](Self::reloaded) banner. Let go once the file
    /// is as the session holds it again.
    reload_asked: bool,
    /// The check out is Reload's: until it answers, a reload of the file the session makes
    /// is the person's, so [`reload_asked`](Self::reload_asked) is kept.
    reload_out: bool,
    /// The revision the newest reload the tab saw came to. A `status` of an older revision
    /// compared the file with the bytes that reload replaced.
    reloaded_to: Option<Revision>,
    /// What's selected (M5.7): features of the timeline or the browser, faces and edges of
    /// the canvas, by name; and what the pointer is over.
    selection: crate::selection::Selection,
    /// The canvas's box selection (M5.7): a left drag, on geometry or empty space.
    box_select: crate::box_select::BoxSelect,
    /// The canvas's Select Other (M5.7): its right click's menu, and the list of what's
    /// under the pointer.
    select_other: crate::select_other::SelectOther,
    /// What the core says of the selection's elements, for the property panel's selection
    /// view (M5.7's measures): one element's entry, or two's distance and angle.
    measures: crate::measures::Measures,
    /// What a toast is to say for the tab (a box's count, or why it took nothing), until
    /// the app takes it.
    toast: Option<String>,
    /// The `diff` of the reload [`reloaded`](Self::reloaded) shows, asked as the reload was
    /// seen, from the revision the tab had to the session's now, until it answers.
    reload_diff: Option<Running<DiffAnswer>>,
    /// The features another program changed, each by its ID with what changed in it, as
    /// the reloads' diffs answered (`d.outside`): marked purple in the timeline and the
    /// browser until the person marks it seen in its properties (DESIGN.md, 6c).
    pub outside: BTreeMap<FeatureId, String>,
    /// Show changes (DESIGN.md, 6d): what another program wrote to the file and what the
    /// edits held changed, each as its own `diff` answers ([`HeldChanges`]); none until
    /// it's asked, and again once its modal closes.
    pub held_changes: Option<HeldChanges>,
    /// Show changes' two diffs, the file's and the edits', each until it answers.
    theirs_diff: Option<Running<DiffAnswer>>,
    mine_diff: Option<Running<DiffAnswer>>,
    /// What the browser's eyes hide: the tab's state, never saved (open item 29).
    pub hidden: std::collections::BTreeSet<Hideable>,
    /// The last refusal a command of the tab's answered, as said to the person, until
    /// their next action (an edit, a save, a revert, a step).
    pub message: Option<String>,
    /// What the last suppression or deletion did, as said to the person (the prototype's
    /// toast, in the status bar): what it suppressed with the feature, or rewired in its
    /// place. Until their next action, as [`message`](Self::message) is.
    pub told: Option<String>,
    /// A feature's name being typed in its timeline item's or browser row's own field (F2,
    /// or a menu's Rename), until Enter or a click away sends it or Esc drops it.
    pub renaming: Option<crate::feature_menu::Renaming>,
    /// Where the person last selected a feature: F2 renames it there.
    pub selected_in: crate::feature_menu::Place,
    /// A save or a revert that was to close the tab was refused: the tab stays, and a quit
    /// waiting on it stops ([`take_close_refused`](Self::take_close_refused)).
    close_refused: bool,
    /// A Save As found a file at the path it was given, and wasn't to replace it: the path,
    /// and whether the Save As was to close the tab, for the window to ask whether to
    /// ([`take_replace_asked`](Self::take_replace_asked)).
    replace_asked: Option<(String, bool)>,
    /// A Save As that began its own transaction was refused: it's rolled back next.
    roll_back: bool,
    /// The session has closed: the tab goes.
    pub closed: bool,
    /// The feature dialog open (section 9, *Feature dialogs*), if one is; and the number of
    /// the newest opened, and of one whose feature is still being read (Edit Feature).
    pub dialog: Option<Dialog>,
    dialogs: u64,
    loading: Option<u64>,
    /// The revision the dialog opened at, once everything asked before it has answered,
    /// and whether no transaction was open then (its own edits begin it).
    dialog_base: Option<Revision>,
    dialog_began: bool,
    /// Sketch mode (section 9, *Sketching*), while a sketch is open; and each sketch's
    /// state, for the browser's prefixes.
    pub sketch: Option<SketchMode>,
    pub prefixes: Prefixes,
    /// The selection as it was before the open dialog's box of names took it, put back if
    /// the dialog is cancelled, as SolidWorks returns to the selection its command started
    /// with.
    before_dialog: Option<Vec<crate::selection::Item>>,
    /// The last dialog OK'd, for the window to remember its values ([`take_oked`]
    /// (Self::take_oked), [`crate::dialog::Remembered`]).
    oked: Option<Dialog>,
    /// The dialogs' edits OK'd in this transaction, each one step for undo and redo.
    groups: Vec<Group>,
    /// The state sketch mode's Finish or Cancel ended at, or an over-defining dimension's
    /// Cancel undid to: Redo is off there till the next edit, so what it undid isn't redone
    /// past its Finish, nor the dimension Cancel refused.
    redo_fence: Option<Revision>,
    /// Whether an edit of the tab's was applied since the last dialog or sketch mode
    /// opened: whether a sketch mode's Finish or Cancel has anything of its own to fence.
    sketch_edited: bool,
    /// Each revision an undo or a redo came to, and the first revision of the state it
    /// restored: what names a state, whichever revision the session is at it by.
    restored: BTreeMap<Revision, Revision>,
    /// The fields whose last edit was refused, by feature, since the dialog opened: its
    /// input of one is red, and its OK off, until an edit of it is taken.
    refused: std::collections::BTreeSet<(FeatureId, String)>,
    /// The commands it sent that change its transaction, in order, the newest
    /// [`SENT_KEPT`]: the call record.
    pub sent: Vec<Sent>,
    /// The export running (M5.12), as it was asked, and its answer to come; each export the
    /// tab sent, as sent (the call record); and the last one's answer, once it came.
    exporting: Option<(Export, Running<Exported>)>,
    /// The newest [`SENT_KEPT`] exports sent.
    pub exports: Vec<Export>,
    pub export_answer: Option<Result<Exported, Problems>>,
    /// What the export dialog says: the last export's refusal, as said to the person; the
    /// request whose path held a file, to ask whether to replace it; and whether the last
    /// export wrote its file, until the window takes that (closing the dialog).
    pub export_said: Option<String>,
    pub export_replacing: Option<Export>,
    exported: bool,
    /// The export out was sent from a dialog since cancelled, not the one open now.
    export_disowned: bool,
    /// The broken reference's candidate whose card row the pointer is over (flow 6b), as
    /// the frame before had it, which the canvas lights; and this frame's, as the cards
    /// drawn so far have said it ([`hover_candidate`](Self::hover_candidate)).
    candidate_hot: Option<String>,
    candidate_next: Option<String>,
    /// The viewport's camera, meshes, and `meshes` read (M5.6).
    pub view: crate::viewport::View,
    /// Sketch mode's drag, from its press to the read of its edit: the tab's, not sketch
    /// mode's, so what waits behind it (the queue, Finish's and Cancel's close) still sees
    /// it once sketch mode has closed. And the drag as its begin was sent: what its end's
    /// edit is told against.
    pub drag: Option<Drag>,
    drag_sending: Option<Sending>,
    /// A test's watch on what [`overlay`](Self::overlay) is given: called after it each
    /// frame with the same painter and frame. For the crate's tests only; None otherwise.
    #[doc(hidden)]
    pub watch: Option<Watch>,
    /// A test's hook in [`poll`](Self::poll), called once the session's `status` is read
    /// and before any answer is taken: so a test can have commands answer between the two,
    /// as they may in any frame. For the crate's tests only; None otherwise.
    #[doc(hidden)]
    pub after_status: Option<AfterStatus>,
}

/// What an over-defining dimension's refused choice says: [`OVERTAKEN`] for a `conflict`,
/// else the refusal, as [`said`] words it.
fn overtaken_or(problems: &Problems) -> String {
    if problems
        .errors()
        .first()
        .is_some_and(|error| error.code() == ErrorCode::Conflict)
    {
        OVERTAKEN.to_owned()
    } else {
        said(problems)
    }
}

/// What a test sees of each frame a tab's overlay is given, and draws with its painter:
/// for the crate's tests only.
#[doc(hidden)]
pub type Watch = Box<dyn FnMut(&eframe::egui::Painter, &crate::viewport::ViewFrame)>;

/// What a test does in a tab's [`poll`](Tab::poll) between its `status` read and the
/// answers it takes: for the crate's tests only.
#[doc(hidden)]
pub type AfterStatus = Box<dyn FnMut()>;

/// A feature's name as the document reads a string (`set_field`'s value).
pub(crate) fn name_value(name: &str) -> Result<Node, silico_core::document::json::SyntaxError> {
    silico_core::document::json::read(crate::dialog::quoted(name).as_bytes())
}

/// What a refusal says to the person: its first error's message and suggestion.
pub fn said(problems: &Problems) -> String {
    problems.errors().first().map_or_else(
        || "refused".to_owned(),
        |error| match error.suggestion() {
            Some(suggestion) => format!("{} ({suggestion})", error.message()),
            None => error.message().to_owned(),
        },
    )
}

impl Tab {
    /// The tab of a session `open` opened, with the gate's problems it found.
    pub fn opened(id: SessionId, path: String, problems: &Problems) -> Self {
        Self::of(id, Some(path), String::new(), problems)
    }

    /// The tab of an untitled part (`open` with no path), shown as `name` until its first
    /// save gives it a file.
    pub fn untitled(id: SessionId, name: String, problems: &Problems) -> Self {
        Self::of(id, None, name, problems)
    }

    /// The tab of a session, its file at `path` (none while untitled, shown as `untitled`).
    fn of(id: SessionId, path: Option<String>, untitled: String, problems: &Problems) -> Self {
        Tab {
            id,
            path,
            untitled,
            revision: Revision::OPENED,
            transaction: false,
            status: None,
            status_revision: Revision::OPENED,
            rebuilt: None,
            published: None,
            published_model: None,
            published_models: 0,
            timeline: None,
            browser: Browser::default(),
            timeline_read: None,
            timeline_superseded: false,
            timeline_asked: None,
            nothing_published: false,
            rebuild_out: false,
            said: BTreeMap::new(),
            document_problems: problems.errors().iter().map(Said::of).collect(),
            listed: Vec::new(),
            unlisted: Unlisted::default(),
            listed_for: None,
            whats_wrong: false,
            timeline_scroll: None,
            check_read: None,
            check_superseded: false,
            check_first: false,
            fresh_out: false,
            checked: None,
            edits: Edits::default(),
            under: Vec::new(),
            saves: 0,
            rebuilds: 0,
            rebuilds_all: 0,
            resets: 0,
            beginning: false,
            disk_choice: DiskChoice::None,
            reloaded: None,
            reload_asked: false,
            reload_out: false,
            reloaded_to: None,
            selection: crate::selection::Selection::default(),
            box_select: crate::box_select::BoxSelect::default(),
            select_other: crate::select_other::SelectOther::default(),
            measures: crate::measures::Measures::default(),
            toast: None,
            reload_diff: None,
            outside: BTreeMap::new(),
            held_changes: None,
            theirs_diff: None,
            mine_diff: None,
            hidden: std::collections::BTreeSet::new(),
            message: None,
            told: None,
            renaming: None,
            selected_in: crate::feature_menu::Place::Timeline,
            close_refused: false,
            replace_asked: None,
            roll_back: false,
            closed: false,
            dialog: None,
            dialogs: 0,
            loading: None,
            dialog_base: None,
            dialog_began: false,
            sketch: None,
            prefixes: Prefixes::default(),
            before_dialog: None,
            oked: None,
            groups: Vec::new(),
            redo_fence: None,
            sketch_edited: false,
            restored: BTreeMap::new(),
            refused: std::collections::BTreeSet::new(),
            sent: Vec::new(),
            exporting: None,
            exports: Vec::new(),
            export_answer: None,
            export_said: None,
            export_replacing: None,
            exported: false,
            export_disowned: false,
            candidate_hot: None,
            candidate_next: None,
            view: crate::viewport::View::default(),
            drag: None,
            drag_sending: None,
            watch: None,
            after_status: None,
        }
    }

    /// What the tab's mode draws over the viewport's bodies, and acts on, each frame the
    /// tab is shown (sketch mode, M5.7's selection): it acts on `frame.left`, the left
    /// button's events on the canvas, and `frame.secondary`, the right button's clicks
    /// there (opening its own menu, if it has one), and draws with `painter`, the pane's
    /// own layer, clipped to the viewport, over the bodies and under the ViewCube and the
    /// navigation bar. The end of a drag in a tab no longer shown comes here too, its
    /// painter drawing nothing. A mode sends its commands from here through `sessions`, on
    /// the tab's own queue and its `Running` handles, polled with `try_take`, never waiting:
    /// a drag's move goes out in the frame it happened.
    ///
    /// While a sketch is open, they're sketch mode's: the selection's canvas is claimed
    /// (a click of the selection's waiting let go, and a box drawn), Select Other's menu
    /// closed (one open, or a right click waiting for its boxes, as the sketch opened, let
    /// go, and the hover, the wheel, and the window's keys with it), and sketch mode's frame
    /// ([`sketch_frame`](Self::sketch_frame)) is given the frame's events and pointer in
    /// points, and its sketch's plane on the screen and back through the frame's camera
    /// ([`Screen`]). SEAM: a face pick for Create Sketch, reading
    /// [`selection`](Self::selection)'s hover and pick.
    ///
    /// With no sketch open, the left button and the pointer are the selection's (M5.7):
    /// a click picks what's under it, the hover what the pointer is over, a drag on
    /// geometry what's under its press, and a drag from empty space a box
    /// (`crate::box_select`), each read against the published model the tab last polled,
    /// which asks the session nothing more. The box select comes after the selection, so
    /// it reads the hover this frame's answer gave. A right click opens the canvas's context
    /// menu (`crate::canvas_menu`, through `crate::select_other`), first of them: while it's
    /// open it holds the hover and the wheel, and a left press that closes it is given to
    /// neither; an item chosen in it is done here, as its command is elsewhere.
    pub fn overlay<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        painter: &eframe::egui::Painter,
        frame: &crate::viewport::ViewFrame,
    ) {
        if let Some(mode) = self.sketch.as_ref() {
            self.selection.canvas_claimed();
            self.box_select = Default::default();
            self.select_other
                .close(&mut self.selection, &mut self.view, painter.ctx());
            let (ppp, origin) = (frame.pixels_per_point, frame.rect.left_top());
            let events = sketch::frame_events(&frame.left, &frame.secondary, ppp, origin);
            let pointer = frame
                .pointer_px
                .map(|at_px| sketch::at_points(at_px, ppp, origin));
            let screen = mode.drawable().and_then(|read| Screen::of(read, frame));
            let project = |at| screen.as_ref().and_then(|screen| screen.project(at));
            let to_plane = |at| screen.as_ref().and_then(|screen| screen.to_plane(at));
            let input = FrameInput {
                events: &events,
                pointer,
                project: &project,
                to_plane: &to_plane,
            };
            self.sketch_frame(sessions, painter, &input);
            return;
        }
        // The bodies are of the model the selection is read against: the one published
        // last, as counted.
        let current = matches!(
            (self.published, self.view.bodies_of()),
            (Some(revision), Some(of)) if of == (self.published_models, revision)
        );
        // Select Other first (`crate::select_other`): while its menu is open it holds the
        // hover and the wheel, and a left press that closes it is its own, given to neither
        // the selection nor the box. Sketch mode claims the canvas before this, so Select
        // Other, the selection's, never opens in a sketch (one open, or waiting, as a sketch
        // opens is closed there).
        // Why Edit can't open each feature's dialog, as the feature menus say it: worked out
        // only in a frame a right click may open the menu in.
        let edit_off: BTreeMap<silico_core::document::FeatureId, Option<String>> =
            if frame.secondary.is_empty() {
                BTreeMap::new()
            } else {
                let entries = self
                    .timeline
                    .as_ref()
                    .and_then(|timeline| timeline.features.as_deref())
                    .unwrap_or_default();
                entries
                    .iter()
                    .map(|entry| {
                        let off = crate::feature_menu::off(
                            self,
                            entry,
                            crate::feature_menu::Item::Edit,
                            crate::feature_menu::Place::Browser,
                        );
                        (entry.id.clone(), off)
                    })
                    .collect()
            };
        let entries = self
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_deref())
            .unwrap_or_default();
        let names = |id: &silico_core::document::FeatureId| {
            entries
                .iter()
                .find(|entry| &entry.id == id)
                .map(|entry| entry.name.clone())
        };
        let edit = |id: &silico_core::document::FeatureId| {
            edit_off
                .get(id)
                .cloned()
                .unwrap_or_else(|| Some("Its feature isn't in the timeline yet".to_owned()))
        };
        let facts = crate::select_other::Facts {
            names: &names,
            edit_off: &edit,
        };
        let (left, asked) = self.select_other.overlay(
            &mut self.selection,
            &mut self.view,
            frame,
            painter,
            current,
            &facts,
        );
        let passed;
        let frame = if left.len() == frame.left.len() {
            frame
        } else {
            passed = crate::viewport::ViewFrame {
                left,
                ..frame.clone()
            };
            &passed
        };
        self.selection.overlay(&mut self.view, frame, painter.ctx());
        let hidden: std::collections::BTreeSet<silico_core::document::FeatureId> = self
            .hidden
            .iter()
            .filter_map(|what| match what {
                Hideable::Feature(id) => Some(id.clone()),
                Hideable::Origin(_) => None,
            })
            .collect();
        if let Some(toast) = self.box_select.overlay(
            &mut self.selection,
            &mut self.view,
            frame,
            painter,
            current,
            &hidden,
        ) {
            self.toast = Some(toast);
        }
        let candidate = self
            .candidate_hot
            .as_deref()
            .and_then(|name| self.selection.lit(name, &self.view));
        if candidate != self.view.candidate {
            // Drawn in the next frame: asked for, the pointer maybe still.
            self.view.candidate = candidate;
            painter.ctx().request_repaint();
        }
        if self.box_toggled() {
            // What the box took is sent from the dialog's pane, maybe drawn already.
            painter.ctx().request_repaint();
        }
        if let Some(asked) = asked {
            self.canvas_act(sessions, asked);
            painter.ctx().request_repaint();
        }
    }

    /// What the canvas's context menu asked, done as its command is elsewhere: Edit as the
    /// feature menus' Edit feature (`crate::feature_menu::act`), Hide as H hides a body, Fit
    /// and Home view as F6 and Home, and Clear selection as Esc clears it.
    fn canvas_act<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        asked: crate::canvas_menu::CanvasAct,
    ) {
        use crate::canvas_menu::CanvasAct;
        use crate::viewport::ViewAsk;
        match asked {
            CanvasAct::Edit(id) => {
                crate::feature_menu::act(self, sessions, crate::feature_menu::Act::Edit(id));
            }
            CanvasAct::Hide(id) => self.toggle_shown(Hideable::Feature(id)),
            CanvasAct::Fit => self.view.ask(ViewAsk::Fit),
            CanvasAct::Home => self.view.ask(ViewAsk::Named(crate::navigate::HOME)),
            CanvasAct::ClearSelection => self.selection.clear(),
        }
    }

    /// The canvas's Select Other menu closed, if it's open: a modal or the shortcut sheet has
    /// come up over it, and takes the keyboard's focus and its keys.
    pub(crate) fn close_select_other(&mut self, ctx: &eframe::egui::Context) {
        self.select_other
            .close(&mut self.selection, &mut self.view, ctx);
    }

    /// What clicks toggled in the selection while the dialog's box took the canvas's picks,
    /// taken into the box. A box of names (`Dialog::toggled`): a pick on the body the
    /// dialog's feature is built into read against the box's base (the timeline's chains say
    /// which bodies are, [`built_through`]); each the box refused, or removed as the click
    /// added it to the selection, let go from the selection again, so the canvas shows what
    /// the box holds. A box of one body (`Dialog::took_body`): each body picked once, in the
    /// order picked (a box drawn over a body's edges picks it once), where it stands to the
    /// dialog's feature ([`standing`]), a pick on the feature's own body read against the
    /// bodies its boxes hold (`Selection::holds_in`); and the selection then the bodies the
    /// boxes hold, each lit whole. Whether any was.
    fn box_toggled(&mut self) -> bool {
        use crate::dialog::Taken;
        let toggled = self.selection.take_toggled();
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        let entries = self
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_deref())
            .unwrap_or_default();
        let own = dialog.feature.clone();
        if dialog.takes_bodies() {
            // A box of one body: each body picked once, in the order picked (a box drawn over
            // a body's edges picks it once), and the selection the bodies the boxes hold.
            let mut seen: Vec<&FeatureId> = Vec::new();
            for toggled in &toggled {
                if let Some(body) = &toggled.body {
                    if seen.contains(&body) {
                        continue;
                    }
                    seen.push(body);
                }
                let standing = toggled.body.as_ref().map_or(Standing::Before, |body| {
                    standing(entries, body, own.as_ref())
                });
                let selection = &self.selection;
                let holds = |body: &FeatureId| selection.holds_in(&toggled.name, body);
                dialog.took_body(toggled, standing, &holds);
            }
            if !toggled.is_empty() {
                self.selection.select_features(dialog.bodies());
            }
            return !toggled.is_empty();
        }
        for toggled in &toggled {
            let through_own = match (&own, &toggled.body) {
                (Some(own), Some(body)) => built_through(entries, body, own),
                _ => false,
            };
            let selection = &self.selection;
            let on_base = |base: &FeatureId| selection.read_on(&toggled.name, base);
            match dialog.toggled(toggled, through_own, &on_base) {
                Taken::Refused => self.selection.let_go(&toggled.name),
                Taken::Removed if toggled.added => self.selection.let_go(&toggled.name),
                Taken::Added | Taken::Removed | Taken::Ignored => {}
            }
        }
        !toggled.is_empty()
    }

    /// What a toast is to say for the tab, taken once (the app shows it).
    pub fn take_toast(&mut self) -> Option<String> {
        self.toast.take()
    }

    /// The canvas's Select Other (M5.7): the menu open, if one is, and its rows.
    pub fn select_other(&self) -> &crate::select_other::SelectOther {
        &self.select_other
    }

    /// `key` for Select Other's menu, taken by the window's keys while it's open (or a right
    /// click waits to open it), before the pane draws: applied in this frame's overlay.
    pub(crate) fn select_other_key(&mut self, key: crate::select_other::MenuKey) {
        self.select_other.key(key);
    }

    /// Select Other's boxes made by `build` from now on ([`crate::App::boxing`]).
    pub(crate) fn boxing(&mut self, build: crate::boxes::Build) {
        self.select_other.building(build);
    }

    /// What's selected (M5.7), and what the pointer is over in the canvas: one selection,
    /// the browser's, the timeline's, and the canvas's.
    pub fn selection(&self) -> &crate::selection::Selection {
        &self.selection
    }

    /// The selection, to change: a pick by name, a feature, a clear, its filter.
    pub fn selection_mut(&mut self) -> &mut crate::selection::Selection {
        &mut self.selection
    }

    /// What the core says of the selection's elements (M5.7's measures), as last answered.
    pub fn measures(&self) -> &crate::measures::Measures {
        &self.measures
    }

    /// The measures of the selection as it is now asked, if they're not what was asked last
    /// (a pick or a rebuild since), and their reads out polled; none asked while a dialog
    /// holds the panel (or is being read), which shows no measures, and whose preview's
    /// rebuild a distance would wait behind.
    pub fn measure<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        self.measures.poll(sessions, &self.id);
        if self.in_dialog() {
            return;
        }
        let of = crate::measures::Of::now(&self.selection);
        // The same version of the selection on the same model: the same elements, as they
        // were found, so nothing more to ask (and nothing to work out each frame).
        if of == self.measures.of() {
            return;
        }
        let measured = crate::measures::Measured::of(&self.selection);
        self.measures.ask(sessions, &self.id, of, measured);
    }

    /// The measures, for a crate's test to answer as if read (`Measures::answered_as`).
    #[cfg(test)]
    pub(crate) fn measures_mut(&mut self) -> &mut crate::measures::Measures {
        &mut self.measures
    }

    /// Whether anything is wrong: the gate's problems, or a feature's problems or warnings,
    /// as its timeline marks them (What's Wrong lists them).
    pub fn anything_wrong(&self) -> bool {
        self.timeline.as_ref().is_some_and(marked)
    }

    /// The core's time for the rebuild of the revision `status` says is rebuilt, in whole
    /// milliseconds: that revision's last rebuild done whole, if one was seen, never
    /// another's (a stopped one's, or another revision's model's).
    pub fn rebuilt_ms(&self) -> Option<u64> {
        let revision = self.status.as_ref()?.rebuild.revision;
        self.rebuilt
            .filter(|(of, _)| *of == revision)
            .map(|(_, ms)| ms)
    }

    /// The errors its timeline counts: the gate's, and each feature's problems (the
    /// timeline's summary and the status bar's).
    pub fn errors(&self) -> u64 {
        self.timeline.as_ref().map_or(0, |timeline| {
            timeline
                .features
                .iter()
                .flatten()
                .map(|entry| entry.problems)
                .fold(timeline.problems, u64::saturating_add)
        })
    }

    /// The tab's name: its file's, or, while it's untitled, the name it was given.
    pub fn name(&self) -> &str {
        match &self.path {
            Some(path) => std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path),
            None => &self.untitled,
        }
    }

    /// Whether it's an untitled part, with no file until its first save (a Save As).
    pub fn is_untitled(&self) -> bool {
        self.path.is_none()
    }

    /// Whether the document differs from the file: `status.transaction.changed` (#300),
    /// false again once undone or edited back to what `begin` saw. Until a status of the
    /// tab's own newest revision is read (an answer came after the last), whether a
    /// transaction is open, so an edit just answered is never taken for none.
    fn changed(&self) -> bool {
        if self.revision != self.status_revision {
            return self.transaction;
        }
        self.transaction()
            .is_some_and(|transaction| transaction.changed)
    }

    /// The transaction open, as the newest `status` gives it: its counts of steps.
    pub fn transaction(&self) -> Option<&silico_core::command::Transaction> {
        self.status
            .as_ref()
            .and_then(|status| status.transaction.as_ref())
    }

    /// Whether the newest `status` is of the tab's own newest revision: no answer came
    /// after it, so its transaction's counts are the session's now.
    fn synced(&self) -> bool {
        self.revision == self.status_revision
    }

    /// Whether an undo or a redo is queued or running.
    fn stepping(&self) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Step { .. }))
            || self
                .under
                .iter()
                .any(|under| matches!(under, Under::Step { .. }))
    }

    /// Whether Undo can step: an edit still to answer (the undo waits behind it), or an
    /// edit to undo as the status of the tab's newest revision counts them; never while
    /// another step is under way, whose answer moves the counts, nor while a dialog is open,
    /// whose Cancel undoes its edits, nor while sketch mode's drag is under way (from its
    /// press to its end's answer, [`Drag::under_way`]), whose end lands an edit of its own;
    /// once that has answered, its edit is undone as any other's, whether or not the read of
    /// it ever comes (its rebuild stopped, or failing). In sketch mode,
    /// as SolidWorks allows, but never past the state it opened at.
    pub fn undoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
            && !self.drag.as_ref().is_some_and(Drag::under_way)
            && self.sketch_lets_undo()
            && !self.stepping()
            && (!self.edits.idle()
                || (self.synced()
                    && self
                        .transaction()
                        .is_some_and(|transaction| transaction.undo > 0)))
    }

    /// Whether Redo can step: an edit undone, and none made since, as the status of the
    /// tab's newest revision counts them; never while a step is under way, nor while a
    /// dialog is open or a drag is under way. In sketch mode, only what it undid itself;
    /// never at a state Redo is fenced off at (a sketch mode's Finish or Cancel, or an
    /// over-defining dimension's Cancel, which leave nothing to redo).
    pub fn redoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
            && !self.drag.as_ref().is_some_and(Drag::under_way)
            && self.sketch.as_ref().is_none_or(|mode| mode.undone > 0)
            && self
                .redo_fence
                .is_none_or(|fence| fence != self.state_of(self.revision))
            && self.edits.idle()
            && !self.stepping()
            && self.synced()
            && self
                .transaction()
                .is_some_and(|transaction| transaction.redo > 0)
    }

    /// Whether sketch mode, if it's open, lets an undo step: it has opened (everything asked
    /// before it has answered), and the session isn't at the state it opened at (for Create
    /// Sketch, the state its add came to).
    fn sketch_lets_undo(&self) -> bool {
        let Some(mode) = &self.sketch else {
            return true;
        };
        mode.undo_floor
            .or(mode.opened_at)
            .is_some_and(|floor| self.state_of(self.revision) != self.state_of(floor))
    }

    /// The name of the sketch open, as its browser row gives it ("Sketch" until it's added
    /// and listed): what the sketch bar and the status bar call it.
    pub fn sketch_name(&self) -> String {
        self.sketch
            .as_ref()
            .and_then(|mode| mode.sketch.as_ref())
            .and_then(|id| self.browser.sketches.iter().find(|row| &row.id == id))
            .map_or_else(|| "Sketch".to_owned(), |row| row.name.clone())
    }

    /// The revision sketch mode opened at, once everything asked before it has answered:
    /// what its Cancel undoes to, and its undo goes no further than.
    pub fn sketch_opened_at(&self) -> Option<Revision> {
        self.sketch.as_ref().and_then(|mode| mode.opened_at)
    }

    /// Whether it holds unsaved edits: the document changed, an edit, a move, or an add under
    /// way, or a transaction open with anything of the tab's still to answer (an undo or a
    /// redo queued or running among them). Only a tab with
    /// none of these is closed without asking (rolled back first if a transaction is open).
    pub fn unsaved(&self) -> bool {
        // A `begin` under way goes with the edit it was sent for (`edits.idle` is false), or
        // with the move or the add it was sent for (`changing`).
        self.changed()
            || !self.edits.idle()
            || self.changing()
            || (self.transaction && !self.quiet())
    }

    /// Whether a move, a dialog's add, Restore's edit, sketch mode's add, edit, dimension's
    /// value, or over-defining dimension's choice, or a drag is queued or running: each
    /// changes the document, and may be under way before the transaction its `begin` opens
    /// is. Restore's read of the text it puts back changes nothing: until it answers, a file
    /// changed on disk is no reason to offer to keep edits. Every kind queued and under way
    /// is named, so a new one is weighed here.
    fn changing(&self) -> bool {
        let queued = self.edits.queue.iter().any(|queued| match queued {
            Queued::Move { .. }
            | Queued::Suppress { .. }
            | Queued::Remove { .. }
            | Queued::Add { .. }
            | Queued::Replace { .. }
            | Queued::AddSketch { .. }
            | Queued::SketchRemove { .. }
            | Queued::SketchJoin { .. }
            | Queued::SketchAdd { .. }
            | Queued::SketchTrim { .. }
            | Queued::SketchSplit { .. }
            | Queued::SketchExtend { .. }
            | Queued::DimensionValue { .. }
            | Queued::Offer { .. } => true,
            // An edit is `edits.idle`'s; a save, a revert, Reload's rebuild, a step, a load, a
            // dialog's or sketch mode's opening or closing, and Restore's read change nothing
            // of their own.
            Queued::Edit(_)
            | Queued::Save { .. }
            | Queued::SaveAs { .. }
            | Queued::Revert { .. }
            | Queued::Reload
            | Queued::Step { .. }
            | Queued::Opened { .. }
            | Queued::Load { .. }
            | Queued::Closed { .. }
            | Queued::Restore { .. } => false,
        });
        let under = self.under.iter().any(|under| match under {
            Under::Move { .. }
            | Under::Suppress { .. }
            | Under::Remove { .. }
            | Under::Add { .. }
            | Under::Base { .. }
            | Under::Replace { .. }
            | Under::AddSketch { .. }
            | Under::SketchEdit { .. }
            | Under::SketchJoin { .. }
            | Under::SketchAdd { .. }
            | Under::SketchCut { .. }
            | Under::DimensionValue { .. }
            | Under::OfferField { .. } => true,
            Under::Begin(_)
            | Under::Commit { .. }
            | Under::Rollback { .. }
            | Under::Step { .. }
            | Under::SaveAs { .. }
            | Under::Close(_)
            | Under::Load { .. }
            | Under::Restore { .. } => false,
        });
        // A drag's begin, moves, or end under way: its edit lands as it ends.
        queued || under || self.dragging()
    }

    /// Save As (`commit` with a path, docs/design/m5.0-app.md §4): once everything asked
    /// before it has answered, as a save, the document written to `path` (replacing a file
    /// there if `replace`), the tab then of the new file; then, if `then_close`, closed (an
    /// untitled part saved by Close's or Quit's Save). A file there and no `replace` is
    /// asked about ([`take_replace_asked`](Self::take_replace_asked)); any other refusal is
    /// said, and, if it was to close the tab, the tab kept and that said too.
    pub fn save_as<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        path: String,
        replace: bool,
        then_close: bool,
    ) {
        self.hush();
        self.edits.queue.push_back(Queued::SaveAs {
            path,
            replace,
            then_close,
        });
        self.advance(sessions);
    }

    /// The path a Save As found a file at, if one did since this was last asked, and
    /// whether that Save As was to close the tab: whether to replace it is the person's to
    /// say.
    pub fn take_replace_asked(&mut self) -> Option<(String, bool)> {
        self.replace_asked.take()
    }

    /// Whether a Save As is queued or running: its answer gives the tab its file, or not.
    pub fn saving_as(&self) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::SaveAs { .. }))
            || self
                .under
                .iter()
                .any(|under| matches!(under, Under::SaveAs { .. }))
    }

    /// Whether a save or a revert that was to close it was refused since this was last asked.
    pub fn take_close_refused(&mut self) -> bool {
        std::mem::take(&mut self.close_refused)
    }

    /// The session's state as it is now (`status` and the published model, each answered at
    /// once), every command under way polled, and the reads a change needs started.
    pub fn poll<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let status = sessions.status(&self.id);
        if let Some(after_status) = &mut self.after_status {
            after_status();
        }
        // Answered at once, of the session as it is: its revision and transaction are now.
        let mut reloaded = false;
        if let Some(session) = &status.session {
            // Moved with nothing of the tab's under way and no transaction open: the
            // session's own reload. A session never reloads inside a transaction, so a move
            // inside one is another client's edit, which a dialog's or sketch mode's Cancel
            // undoes with its own (it opened where it did).
            if self.status.is_some()
                && session.revision != self.revision
                && !session.transaction
                && self.quiet()
            {
                reloaded = true;
                self.reload_seen(sessions, self.revision, session.revision);
            }
            self.revision = session.revision;
            self.transaction = session.transaction;
            self.status_revision = session.revision;
        }
        match status.result {
            Ok(status) => {
                // The file as the session holds it, nothing of the tab's under way: no
                // reload of the person's is still to come.
                // Nor one Reload has yet to read.
                if self.quiet()
                    && !self.reload_queued()
                    && !self.reload_out
                    && !matches!(status.disk, Some(Disk::Changed | Disk::Gone))
                {
                    self.reload_asked = false;
                }
                // A rebuild done whole: its time kept with its revision. One done but not
                // whole (a full rebuild stopped, the revision keeping the model it had: what
                // it hadn't settled left queued, even before its plan, at 0 of 0) or with no
                // rebuild of its own (a save's model, rebound; a rollback's, kept) leaves
                // the last kept.
                if status.rebuild.state == RebuildState::Done
                    && let Some(progress) = status.rebuild.progress.as_ref().filter(|progress| {
                        progress.done == progress.steps
                            && progress.features.iter().all(|(_, state)| {
                                !matches!(state, StepState::Queued | StepState::Running)
                            })
                    })
                {
                    self.rebuilt = Some((status.rebuild.revision, progress.elapsed_ms()));
                }
                self.status = Some(status);
            }
            // Its session isn't open any more: the tab goes.
            Err(_) => {
                self.closed = true;
                return;
            }
        }
        if reloaded {
            self.reloaded_under_dialog(sessions);
        }
        let published = sessions.published(&self.id);
        let model = published
            .result
            .as_ref()
            .ok()
            .map(|published| Arc::downgrade(&published.model));
        // Another model than the last seen, though maybe of the same revision (a fresh
        // check's): counted, so the viewport reads its meshes again.
        let same = match (&self.published_model, &model) {
            (Some(seen), Some(now)) => seen.ptr_eq(now),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.published_models = self.published_models.wrapping_add(1);
            self.published_model = model;
        }
        let model = published
            .result
            .as_ref()
            .ok()
            .map(|published| (Arc::clone(&published.model), published.revision));
        self.selection.published(model, self.published_models);
        self.selection
            .take_into(self.dialog.as_ref().and_then(Dialog::takes));
        self.box_toggled();
        self.published = published.result.ok().map(|published| published.revision);
        if self.published.is_some() {
            self.nothing_published = false;
        }
        self.take_answers(sessions);
        self.offer_disk_choice();
        self.read_timeline(sessions);
        self.read_check(sessions);
        self.sketch_requests(sessions);
        self.drive_drag(sessions);
        self.read_sketches(sessions);
        self.measure(sessions);
    }

    /// The sketch reads polled and started, without waiting: sketch mode's, of its sketch,
    /// and the browser's prefixes', one sketch at a time; each of the newest published
    /// model, read again once that moves.
    fn read_sketches<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.poll(sessions, &self.id, self.published);
            // A drag ended at the newest read's revision or before is drawn by the read.
            if let Some(drag) = self.drag.as_mut() {
                drag.read(mode.read.as_ref().and_then(|read| read.model));
            }
        }
        self.prefixes.poll(
            sessions,
            &self.id,
            &self.browser.sketches,
            self.published,
            self.sketch.as_ref(),
        );
    }

    /// The session's own reload seen, from `from`, the revision the tab had (the one the
    /// reload replaced), to `to`: the views read again; the banner saying so in place of
    /// any before it (none for a reload the person asked for, section 4); and what changed
    /// asked of `diff`, from `from` to `to`, both by number (Fable's decisions of
    /// 2026-11-04, *the three core gaps*, section 1, and *6d's other half*, section 2), so
    /// it reads nothing of the file and a write landing after the reload never refuses it;
    /// its features marked when it answers, the person's reload's too. An older reload's
    /// diff that has answered is taken first, into its own banner; one still out is let
    /// go, so its answer never lands in this banner. An older banner whose changes weren't
    /// listed (its diff still out, or refused: the session no longer held its `from`, a
    /// reload queued ahead of the diff having replaced it) is said in this one, as are
    /// those it carried.
    fn reload_seen<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        from: Revision,
        to: Revision,
    ) {
        self.resets += 1;
        self.reloaded_to = Some(to);
        self.take_diffs();
        let unlisted = match self.reloaded.take() {
            Some(older) => {
                let mut unlisted = older.unlisted;
                if matches!(older.changes, Changes::Asking | Changes::Refused { .. }) {
                    unlisted.push((older.from, older.to));
                }
                unlisted
            }
            None => Vec::new(),
        };
        self.reloaded = (!std::mem::take(&mut self.reload_asked)).then_some(Reloaded {
            from,
            to,
            changes: Changes::Asking,
            refused: None,
            unlisted,
            from_held: true,
        });
        self.reload_diff = Some(sessions.diff(
            &self.id,
            Some(Named::Revision(from)),
            Some(Named::Revision(to)),
        ));
    }

    /// The session's own reload, if `session`, as the answer to a command the tab sent at
    /// revision `sent_at` that changes nothing itself (or one refused) left it, says one
    /// came first: it says it reloaded, or its revision isn't `sent_at`. Seen as
    /// [`poll`](Self::poll) sees one, which it can't while the command is under way.
    fn reload_in<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        sent_at: Revision,
        session: Option<&SessionInfo>,
    ) {
        let Some(session) = session else {
            return;
        };
        if session.reloaded || session.revision != sent_at {
            self.reload_seen(sessions, sent_at, session.revision);
            self.reloaded_under_dialog(sessions);
        }
    }

    /// The session as `response` left it, if that's no older than what the tab knows.
    fn took_session<T>(&mut self, response: &Response<T>) {
        if let Some(session) = &response.session
            && session.revision >= self.revision
        {
            self.revision = session.revision;
            self.transaction = session.transaction;
        }
    }

    fn take_answers<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        self.take_export();
        self.take_diffs();
        if let Some(read) = self.timeline_read.take() {
            match read.try_take() {
                Ok(response) => {
                    // Rebuild's read, if it was, has answered: Rebuild is on again if the
                    // rebuild stops again.
                    self.rebuild_out = false;
                    self.timeline_superseded = false;
                    match response.result {
                        Ok(timeline) => self.took_timeline(timeline),
                        Err(problems) => self.took_refusal(&problems),
                    }
                }
                Err(read) => self.timeline_read = Some(read),
            }
        }
        if let Some((gate, read)) = self.check_read.take() {
            match read.try_take() {
                Ok(response) => self.took_check_answer(gate, response),
                Err(read) => self.check_read = Some((gate, read)),
            }
        }
        let mut close = false;
        for under in std::mem::take(&mut self.under) {
            if let Some(under) = self.take(sessions, under, &mut close) {
                self.under.push(under);
            }
        }
        if close {
            self.under.push(Under::Close(sessions.close(&self.id)));
        }
        if std::mem::take(&mut self.roll_back) {
            self.under.push(Under::Rollback {
                running: sessions.rollback(&self.id),
                then_close: false,
            });
        }
        self.advance(sessions);
    }

    /// The check read out answered, `gate` if it's of a document the gate refused: what it
    /// says marked if it's of the newest model.
    ///
    /// Only a check of the newest model says what's marked: one of an older (its timeline
    /// taken since, this frame or before) would bring back what that timeline let go. The
    /// tab's own check is of the newest timeline's revision and model. Rebuild everything's
    /// is of the current model as it was sent: the newest unless the revision has moved on
    /// since, by an edit the tab sent (`check_superseded`), or by what it didn't send (the
    /// session's reload of its file changed on disk, the tab's rollback, another client's
    /// edit), which the newest timeline's revision, past the one the check read, shows.
    /// The newest's is asked next, if it's wanted.
    fn took_check_answer(&mut self, gate: bool, response: Response<CheckAnswer>) {
        // Reload's, which read the file and found it as the session held it: no reload of
        // the person's is still to come (a Reload waiting behind it marks its own as it's
        // sent).
        if std::mem::take(&mut self.reload_out)
            && !response
                .session
                .as_ref()
                .is_some_and(|session| session.reloaded)
        {
            self.reload_asked = false;
        }
        let superseded = std::mem::take(&mut self.check_superseded);
        let fresh = std::mem::take(&mut self.fresh_out);
        if fresh {
            // Rebuild everything's: what it published is read again.
            self.timeline_asked = None;
        }
        let newest = self
            .timeline
            .as_ref()
            .map(|timeline| (timeline.revision, timeline.model));
        let current = if fresh {
            // The revision of the model it read; of a document the gate refused, which has
            // none, the session's as it answered.
            let read = response
                .model
                .as_ref()
                .map(|model| model.revision)
                .or_else(|| response.session.as_ref().map(|session| session.revision));
            !superseded && read.is_some() && read == newest.map(|(revision, _)| revision)
        } else {
            self.checked.is_some() && self.checked == newest
        };
        if let Ok(answer) = response.result
            && current
        {
            self.took_check(&answer, gate);
        }
    }

    /// The reload's diff and Show changes' two, if they've answered: the reload's listed in
    /// its banner, each feature it changed marked; Show changes' each listed in its section
    /// of the modal, or its refusal said there, each on its own.
    fn take_diffs(&mut self) {
        if let Some(read) = self.reload_diff.take() {
            match read.try_take() {
                Ok(response) => {
                    let changes = match response.result {
                        Ok(answer) => {
                            self.outside.extend(changes::marks(&answer.changes));
                            Changes::Listed(changes::listed(&answer.changes))
                        }
                        Err(problems) => {
                            if let Some(reloaded) = &mut self.reloaded {
                                reloaded.from_held = !changes::not_held(&problems, reloaded.from);
                            }
                            Changes::refused(&problems)
                        }
                    };
                    if let Some(reloaded) = &mut self.reloaded {
                        reloaded.changes = changes;
                    }
                }
                Err(read) => self.reload_diff = Some(read),
            }
        }
        if let Some(changes) = took(&mut self.theirs_diff)
            && let Some(held) = &mut self.held_changes
        {
            held.theirs = changes;
        }
        if let Some(changes) = took(&mut self.mine_diff)
            && let Some(held) = &mut self.held_changes
        {
            held.mine = changes;
        }
    }

    /// Show changes (DESIGN.md, 6d; Fable's decision of 2026-11-04, *6d's other half*,
    /// section 2): two diffs asked for its modal, each from the revision the open
    /// transaction began at: to `"disk"`, the file as the core reads it then (inside the
    /// transaction, reloading nothing and touching no edit), what another program wrote;
    /// and to the document now, what the edits held changed. Neither moves the session's
    /// revision. Nothing with no transaction open.
    pub fn show_changes<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let Some(base) = self.transaction().map(|transaction| transaction.base) else {
            return;
        };
        let base = Some(Named::Revision(base));
        self.held_changes = Some(HeldChanges::default());
        self.theirs_diff = Some(sessions.diff(&self.id, base, Some(Named::Disk)));
        // The edits' side is the document now (`null`), not the tab's newest revision by
        // number: Show changes is offered only inside a transaction, where a `null` side
        // reads no file and reloads nothing, and it names the document as it is when the
        // diff runs, so an edit still in flight can't leave a number no longer held and the
        // list refused (a departure from Fable's `<revision now>`: the same meaning, no
        // race).
        self.mine_diff = Some(sessions.diff(&self.id, base, None));
    }

    /// Show changes' modal closed: its diffs let go.
    pub fn hide_changes(&mut self) {
        self.held_changes = None;
        self.theirs_diff = None;
        self.mine_diff = None;
    }

    /// Restore (Fable's decision of 2026-11-04, section 2; DESIGN.md, 6c): the text of
    /// `revision`, the one the reload replaced, read through `text`, then put back by
    /// `replace_text`, inside the tab's transaction, begun first if none is open: an edit
    /// like any other, which Undo takes back and Save writes. Sent once everything asked
    /// before it has answered. Never while a dialog or a sketch is open (its Cancel would
    /// undo it, and its Finish take it as its own) or the tab closes. A refusal (the
    /// revision no longer held) is said in the reload's banner.
    pub fn restore<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        revision: Revision,
    ) {
        if self.editing() || self.closing() || self.restoring() {
            return;
        }
        self.hush();
        if let Some(reloaded) = &mut self.reloaded {
            reloaded.refused = None;
        }
        self.edits.queue.push_back(Queued::Restore { revision });
        self.advance(sessions);
    }

    /// Whether a Restore is queued or running.
    pub fn restoring(&self) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Restore { .. } | Queued::Replace { .. }))
            || self
                .under
                .iter()
                .any(|under| matches!(under, Under::Restore { .. } | Under::Replace { .. }))
    }

    /// Restore of `revision` refused: said in the banner of the reload that replaced it, if
    /// it still shows, or else in the status bar.
    fn restore_refused(&mut self, revision: Revision, problems: &Problems) {
        let said = changes::refused(problems);
        match &mut self.reloaded {
            Some(reloaded) if reloaded.from == revision => reloaded.refused = Some(said),
            _ => self.message = Some(said),
        }
    }

    /// A timeline read refused: what it says of the published model kept, and asked again
    /// only if that changes what the tab would ask ([`after_refusal`]).
    fn took_refusal(&mut self, problems: &Problems) {
        let (nothing_published, ask_again) = after_refusal(self.nothing_published, problems);
        self.nothing_published = nothing_published;
        if ask_again {
            self.timeline_asked = None;
        }
    }

    /// A timeline read: kept, and the browser's rows worked out from it once.
    fn took_timeline(&mut self, timeline: TimelineAnswer) {
        if !marked(&timeline) {
            self.said.clear();
            self.listed.clear();
            self.unlisted = Unlisted::default();
        } else if timeline.problems == 0 {
            // The gate took it: a feature's text kept only while this read still marks it,
            // so one that healed (an undo, say) says nothing of what it was.
            let still: std::collections::BTreeSet<&str> = timeline
                .features
                .iter()
                .flatten()
                .filter(|entry| entry.problems > 0 || entry.warnings > 0)
                .map(|entry| entry.id.as_str())
                .collect();
            self.said
                .retain(|feature, _| still.contains(feature.as_str()));
        }
        if timeline.problems == 0 {
            self.document_problems.clear();
        }
        self.browser = browser(&timeline);
        self.timeline = Some(timeline);
    }

    /// `under` polled: none if it answered (what it changed taken), or it back.
    fn take<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        under: Under,
        close: &mut bool,
    ) -> Option<Under> {
        match under {
            Under::Begin(running) => match running.try_take() {
                Ok(response) => {
                    self.beginning = false;
                    self.took_session(&response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::Begin(running)),
            },
            Under::Commit {
                running,
                then_close,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.ended(response.result.map(|_| ()), then_close, close);
                    None
                }
                Err(running) => Some(Under::Commit {
                    running,
                    then_close,
                }),
            },
            Under::SaveAs {
                running,
                path,
                began,
                then_close,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    match response.result {
                        // The session holds the new file from now on, by the path given.
                        Ok(committed) => {
                            self.path = Some(committed.path);
                            self.disk_choice = DiskChoice::None;
                            *close |= then_close;
                            if then_close {
                                // Closing: nothing after it is sent.
                                self.edits.queue.clear();
                            }
                        }
                        Err(problems) => {
                            // Its own transaction, begun for it, ended: it held nothing.
                            self.roll_back |= began;
                            let advice = problems.errors().first().and_then(|e| e.suggestion());
                            if advice == Some(silico_core::file::OWN_FILE_ADVICE) {
                                // Its own file, by any path to it (a link, the panel's
                                // canonical form): a Save, sent next, after the rollback of a
                                // transaction it began (nothing to save then, but a close).
                                self.edits.queue.push_front(Queued::Save { then_close });
                            } else if advice == Some(silico_core::file::REPLACE_ADVICE) {
                                // Whether to replace it is asked; a Cancel then refuses the
                                // close, if it was to close.
                                self.replace_asked = Some((path, then_close));
                            } else {
                                self.message = Some(said(&problems));
                                self.close_refused |= then_close;
                            }
                        }
                    }
                    None
                }
                Err(running) => Some(Under::SaveAs {
                    running,
                    path,
                    began,
                    then_close,
                }),
            },
            Under::Rollback {
                running,
                then_close,
            } => match running.try_take() {
                Ok(response) => {
                    // Past its revision already: the session's own reload, once the
                    // transaction ended, seen by `status` before this answer was taken
                    // (nothing of the tab's runs while a rollback does, so nothing else
                    // moves it).
                    if let Some(session) = &response.session
                        && session.revision < self.revision
                    {
                        self.reload_seen(sessions, session.revision, self.revision);
                        self.reloaded_under_dialog(sessions);
                    }
                    self.took_session(&response);
                    // Typing queued after it is applied next: the field keeps it.
                    if response.result.is_ok() && self.edits.idle() {
                        self.resets += 1;
                    }
                    self.ended(response.result.map(|_| ()), then_close, close);
                    None
                }
                Err(running) => Some(Under::Rollback {
                    running,
                    then_close,
                }),
            },
            Under::Step {
                running,
                undo,
                sketch,
                offered,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    // Sketch mode's own step, counted once it's taken: what its Redo may
                    // give back.
                    if let Some(mode) = self
                        .sketch
                        .as_mut()
                        .filter(|mode| response.result.is_ok() && Some(mode.serial) == sketch)
                    {
                        mode.undone = if undo {
                            mode.undone.saturating_add(1)
                        } else {
                            mode.undone.saturating_sub(1)
                        };
                    }
                    if let (Ok(stepped), Some(session)) = (&response.result, &response.session) {
                        let state = self.state_of(stepped.restored);
                        self.restored.insert(session.revision, state);
                        // An over-defining dimension's Cancel (SD7) leaves nothing to redo,
                        // as SolidWorks' does: Redo is off at the state it undid to till the
                        // next edit, so the conflicting dimension never comes back unasked.
                        if offered && undo {
                            self.redo_fence = Some(state);
                        }
                    }
                    match response.result {
                        // Typing queued after it is applied next: the field keeps it.
                        Ok(_) if self.edits.idle() => self.resets += 1,
                        Ok(_) => {}
                        Err(problems) if offered => self.message = Some(overtaken_or(&problems)),
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Step {
                    running,
                    undo,
                    sketch,
                    offered,
                }),
            },
            Under::Add {
                serial,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    let dialog = self
                        .dialog
                        .as_mut()
                        .filter(|dialog| dialog.serial == serial);
                    match response.result {
                        Ok(added) => {
                            if let Some(dialog) = dialog {
                                dialog.added(added.feature.id.clone());
                                // Selected, so its mark's text is read and the property
                                // panel shows it; but not over the elements a box of
                                // names takes from the selection (its mark's text is read
                                // for the dialog's feature, `read_check`).
                                if dialog.takes().is_none() {
                                    self.selection.select_feature(added.feature.id);
                                }
                            }
                        }
                        Err(problems) => {
                            if let Some(dialog) = dialog {
                                dialog.adding = false;
                            }
                            self.message = Some(said(&problems));
                        }
                    }
                    None
                }
                Err(running) => Some(Under::Add {
                    serial,
                    sending,
                    running,
                }),
            },
            Under::Move { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    // A refusal said in the status bar, the timeline as it was.
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::Move { sending, running }),
            },
            Under::Suppress { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    match response.result {
                        Ok(done) => {
                            self.told =
                                crate::feature_menu::suppressed_words(&done, |id| self.name_of(id));
                        }
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Suppress { sending, running }),
            },
            Under::Remove {
                feature,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    match response.result {
                        Ok(removed) => {
                            // Selected no longer: its properties have nothing to show.
                            if self.selection.has_feature(&removed.feature.id) {
                                self.selection.clear();
                            }
                            self.told = Some(crate::feature_menu::removed_words(&removed, |id| {
                                self.name_of(id)
                            }));
                        }
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Remove {
                    feature,
                    sending,
                    running,
                }),
            },
            Under::Base {
                serial,
                kind,
                members,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    // The document's tip at the revision it was read at; read again if the read
                    // was refused (it names no model when none is published, so it's refused
                    // only as every read is).
                    let body = match response.result {
                        Ok(timeline) => Body::Read(
                            timeline.features.as_deref().and_then(tip_of),
                            timeline.revision,
                        ),
                        Err(_) => Body::Unread,
                    };
                    // Sent next, before anything queued after it.
                    self.edits.queue.push_front(Queued::Add {
                        serial,
                        kind,
                        members,
                        body,
                    });
                    None
                }
                Err(running) => Some(Under::Base {
                    serial,
                    kind,
                    members,
                    running,
                }),
            },
            Under::AddSketch {
                serial,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    let ours = self
                        .sketch
                        .as_ref()
                        .is_some_and(|mode| mode.serial == serial);
                    match response.result {
                        Ok(added) => {
                            let at = self.revision;
                            if let Some(mode) = self.sketch.as_mut().filter(|_| ours) {
                                mode.sketch = Some(added.feature.id.clone());
                                mode.undo_floor = Some(at);
                                self.selection.select_feature(added.feature.id);
                            }
                        }
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            // Nothing to edit: sketch mode leaves, as Cancel does, the
                            // transaction its add began, if it did, rolled back.
                            if ours {
                                self.leave_drag();
                                self.sketch = None;
                                self.edits.queue.push_front(Queued::Closed {
                                    ok: false,
                                    sketch: true,
                                });
                            }
                        }
                    }
                    None
                }
                Err(running) => Some(Under::AddSketch {
                    serial,
                    sending,
                    running,
                }),
            },
            Under::SketchEdit { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::SketchEdit { sending, running }),
            },
            Under::SketchJoin { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::SketchJoin { sending, running }),
            },
            Under::SketchAdd {
                serial,
                token,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    // The revision the add made, as the session gave it.
                    let made = response
                        .session
                        .as_ref()
                        .map_or(self.revision, |session| session.revision);
                    let mode = self.sketch.as_mut().filter(|mode| mode.serial == serial);
                    match response.result {
                        Ok(_) => {
                            if let Some(mode) = mode {
                                mode.tools.added(token, made);
                            }
                        }
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            if let Some(mode) = mode {
                                mode.tools.refused(token);
                            }
                        }
                    }
                    None
                }
                Err(running) => Some(Under::SketchAdd {
                    serial,
                    token,
                    sending,
                    running,
                }),
            },
            Under::SketchCut { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::SketchCut { sending, running }),
            },
            Under::DimensionValue {
                serial,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    let made = response
                        .session
                        .as_ref()
                        .map_or(self.revision, |session| session.revision);
                    let answered = match &response.result {
                        Ok(set) => Ok(set.changed),
                        Err(problems) => {
                            self.message = Some(said(problems));
                            Err(said(problems))
                        }
                    };
                    if let Some(mode) = self.sketch.as_mut().filter(|mode| mode.serial == serial) {
                        mode.value_answered(made, answered);
                    }
                    None
                }
                Err(running) => Some(Under::DimensionValue {
                    serial,
                    sending,
                    running,
                }),
            },
            Under::OfferField { sending, running } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    if let Err(problems) = response.result {
                        self.message = Some(overtaken_or(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::OfferField { sending, running }),
            },
            Under::Load {
                serial,
                kind,
                feature,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    if self.loading == Some(serial) {
                        self.loading = None;
                        // The session moved between the dialog's opening and its read (its
                        // reload of a file changed on disk): it opens at the revision it read,
                        // nothing of the tab's having been sent in between.
                        if let Some(session) = &response.session
                            && self
                                .dialog_base
                                .is_some_and(|base| base != session.revision)
                        {
                            self.dialog_base = Some(session.revision);
                            self.dialog_began = !session.transaction;
                        }
                        let node = response
                            .result
                            .map_err(|problems| said(&problems))
                            .and_then(|answer| {
                                silico_core::document::json::read(answer.text.as_bytes())
                                    .map_err(|_| "its text doesn't read as a feature".to_owned())
                            });
                        match node.map(|node| Dialog::load(kind, feature, &node, serial)) {
                            Ok(Some(dialog)) => {
                                // Its box of names holds the feature's own: the selection
                                // (the feature, as Edit Feature selected it) let go, so the
                                // canvas shows what the box takes from it.
                                if let Some(kinds) = dialog.takes() {
                                    self.before_dialog = Some(self.selection.saved());
                                    // A boolean's boxes hold its bodies: selected, so the
                                    // canvas lights what of them it shows.
                                    if dialog.takes_bodies() {
                                        self.selection.select_features(dialog.bodies());
                                    } else {
                                        self.selection.clear();
                                    }
                                    self.selection.take_into(Some(kinds));
                                }
                                self.dialog = Some(dialog);
                            }
                            Ok(None) => {
                                self.message =
                                    Some(format!("Its dialog doesn't read this {}", kind.title()));
                            }
                            Err(message) => self.message = Some(message),
                        }
                    }
                    None
                }
                Err(running) => Some(Under::Load {
                    serial,
                    kind,
                    feature,
                    running,
                }),
            },
            Under::Restore {
                revision,
                sent_at,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    // Its read names a revision, so it reads nothing of the file; but a
                    // reload queued ahead of it (the disk watch's own, or another client's
                    // read) may have run first, which no answer said while it waited:
                    // another program's change, said in place of the banner Restore was
                    // clicked on.
                    self.reload_in(sessions, sent_at, response.session.as_ref());
                    self.took_session(&response);
                    match response.result {
                        // Put back next, before anything queued after it.
                        Ok(answer) => self.edits.queue.push_front(Queued::Replace {
                            revision,
                            text: answer.text,
                        }),
                        Err(problems) => self.restore_refused(revision, &problems),
                    }
                    None
                }
                Err(running) => Some(Under::Restore {
                    revision,
                    sent_at,
                    running,
                }),
            },
            Under::Replace {
                revision,
                began,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    match response.result {
                        Ok(_) => {
                            // The banner's done (the prototype's `undoReload`); the property
                            // panel's field shows the document's value again.
                            if self
                                .reloaded
                                .as_ref()
                                .is_some_and(|reloaded| reloaded.from == revision)
                            {
                                self.reloaded = None;
                            }
                            if self.edits.idle() {
                                self.resets += 1;
                            }
                        }
                        Err(problems) => {
                            // At another revision than it was sent against: the session
                            // reloaded first (its `begin`'s read of the file), which no
                            // answer said while it ran.
                            self.reload_in(sessions, sending.against, response.session.as_ref());
                            // Its own transaction, begun for it, ended: it held nothing,
                            // and, left open, the session would follow the file no more.
                            self.roll_back |= began;
                            self.restore_refused(revision, &problems);
                        }
                    }
                    None
                }
                Err(running) => Some(Under::Replace {
                    revision,
                    began,
                    sending,
                    running,
                }),
            },
            Under::Close(running) => match running.try_take() {
                Ok(response) => {
                    match response.result {
                        Ok(Closed) => self.closed = true,
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            self.close_refused = true;
                        }
                    }
                    None
                }
                Err(running) => Some(Under::Close(running)),
            },
        }
    }

    /// A save's or a revert's answer: the transaction ended, then closed if `then_close`; or
    /// refused, said, and, if it was to close, the tab kept and that said too.
    fn ended(&mut self, result: Result<(), Problems>, then_close: bool, close: &mut bool) {
        match result {
            Ok(()) => {
                self.disk_choice = DiskChoice::None;
                // The transaction's undo stack is gone, and the states it held.
                self.groups.clear();
                self.restored.clear();
                self.redo_fence = None;
                *close |= then_close;
                if then_close {
                    // Closing: nothing after it is sent.
                    self.edits.queue.clear();
                }
            }
            Err(problems) => {
                self.message = Some(said(&problems));
                self.close_refused |= then_close;
            }
        }
    }

    /// The edit in flight polled, and, once nothing runs that what's queued waits behind
    /// (an edit, a save, a revert, a step), the first queued sent, against the revision the
    /// last answer gave. An edit outside a transaction begins one (decision 1). A save or a
    /// revert that comes up with no transaction open (a save before it took every edit, or a
    /// revert ended it) has nothing to do: closes, if it was to close, or else is dropped.
    fn advance<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(Flying {
            field,
            value,
            sending,
            running,
        }) = self.edits.in_flight.take()
        {
            match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    // Said, the field keeping what the person typed (#298's review): a
                    // conflict's too, whose next edit is sent against the reloaded revision.
                    // A box of one body shows what the document holds instead, so it isn't
                    // marked once it does.
                    match response.result {
                        Ok(_) => {
                            self.refused.remove(&field);
                            self.dialog_answered(&field, &value, true);
                        }
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            // What a repair said it did, it didn't.
                            self.told = None;
                            if self.dialog_answered(&field, &value, false) {
                                self.refused.remove(&field);
                            } else {
                                self.refused.insert(field);
                            }
                        }
                    }
                }
                Err(running) => {
                    self.edits.in_flight = Some(Flying {
                        field,
                        value,
                        sending,
                        running,
                    });
                    return;
                }
            }
        }
        while !self.blocked() {
            let Some(queued) = self.edits.queue.pop_front() else {
                return;
            };
            match queued {
                Queued::Edit(edit) => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.edits.sent += 1;
                    self.record(Sent::Edit(edit.field.clone()));
                    let field = (edit.feature.clone(), edit.field.clone());
                    let value = edit.value.clone();
                    let sending = self.sending();
                    let running = sessions.set_field(
                        &self.id,
                        self.revision,
                        edit.feature,
                        edit.field,
                        edit.value,
                    );
                    self.edits.in_flight = Some(Flying {
                        field,
                        value,
                        sending,
                        running,
                    });
                }
                // A sweep's body read at another revision than the session's now: read again.
                Queued::Add {
                    serial,
                    kind,
                    members,
                    body: Body::Read(_, read_at),
                } if kind.sweeps() && read_at != self.revision => {
                    self.edits.queue.push_front(Queued::Add {
                        serial,
                        kind,
                        members,
                        body: Body::Unread,
                    });
                }
                Queued::Add {
                    serial,
                    kind,
                    members,
                    body: Body::Unread,
                } if kind.sweeps() => {
                    let running = sessions.timeline(&self.id, ModelChoice::Published);
                    self.under.push(Under::Base {
                        serial,
                        kind,
                        members,
                        running,
                    });
                }
                Queued::Add {
                    serial,
                    kind,
                    members,
                    body,
                } => {
                    let base = match body {
                        Body::Read(base, _) => base,
                        Body::Unread => None,
                    };
                    let Some(feature) = feature_node(kind, &members, base.as_ref()) else {
                        // Never: the dialog's members are JSON it wrote.
                        if let Some(dialog) = self
                            .dialog
                            .as_mut()
                            .filter(|dialog| dialog.serial == serial)
                        {
                            dialog.adding = false;
                        }
                        self.message = Some(format!("The {} couldn't be written", kind.title()));
                        continue;
                    };
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Add);
                    let sending = self.sending();
                    self.under.push(Under::Add {
                        serial,
                        sending,
                        running: sessions.add_feature(&self.id, self.revision, None, feature),
                    });
                }
                Queued::Move { feature, before } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Move(feature.clone(), before.clone()));
                    let sending = self.sending();
                    self.under.push(Under::Move {
                        sending,
                        running: sessions.move_feature(&self.id, self.revision, feature, before),
                    });
                }
                Queued::Suppress {
                    feature,
                    suppressed,
                } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Suppress(feature.clone(), suppressed));
                    let sending = self.sending();
                    self.under.push(Under::Suppress {
                        sending,
                        running: sessions.suppress_feature(
                            &self.id,
                            self.revision,
                            feature,
                            suppressed,
                        ),
                    });
                }
                Queued::Remove { feature } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Remove(feature.clone()));
                    let sending = self.sending();
                    self.under.push(Under::Remove {
                        feature: feature.clone(),
                        sending,
                        running: sessions.remove_feature(&self.id, self.revision, feature),
                    });
                }
                Queued::Reload => {
                    // One check of the tab's out at a time: one out answers first.
                    if self.check_read.is_some() {
                        self.edits.queue.push_front(Queued::Reload);
                        return;
                    }
                    // The reload its read makes is the person's, whatever a Reload before it
                    // came to (an untitled part's reads no file).
                    self.reload_asked = !self.is_untitled();
                    self.rebuild_all(sessions);
                    if self.check_read.is_some() {
                        self.reload_out = true;
                    } else {
                        // Not sent (a dialog opened meanwhile, or the tab closes): no reload
                        // of the person's comes.
                        self.reload_asked = false;
                    }
                }
                Queued::Opened { serial } => {
                    self.dialog_base = Some(self.revision);
                    self.sketch_edited = false;
                    self.dialog_began = !self.transaction && !self.beginning;
                    // Sketch mode's own opening, never another's: what its undo stops at.
                    if let Some(mode) = self.sketch.as_mut().filter(|mode| mode.serial == serial) {
                        mode.opened_at = Some(self.revision);
                    }
                }
                Queued::AddSketch { serial, feature } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Add);
                    let sending = self.sending();
                    self.under.push(Under::AddSketch {
                        serial,
                        sending,
                        running: sessions.add_feature(&self.id, self.revision, None, feature),
                    });
                }
                Queued::SketchRemove { sketch, members } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_remove"));
                    let sending = self.sending();
                    let running = sessions.sketch_remove(&self.id, self.revision, sketch, members);
                    self.under.push(Under::SketchEdit { sending, running });
                }
                Queued::SketchJoin { sketch, point } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_join"));
                    let sending = self.sending();
                    let running = sessions.sketch_join(&self.id, self.revision, sketch, point);
                    self.under.push(Under::SketchJoin { sending, running });
                }
                Queued::SketchAdd {
                    serial,
                    sketch,
                    token,
                    members,
                } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_add"));
                    let sending = self.sending();
                    let running = sessions.sketch_add(&self.id, self.revision, sketch, members);
                    self.under.push(Under::SketchAdd {
                        serial,
                        token,
                        sending,
                        running,
                    });
                }
                Queued::SketchTrim {
                    sketch,
                    curve,
                    pick,
                } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_trim"));
                    let sending = self.sending();
                    let running =
                        sessions.sketch_trim(&self.id, self.revision, sketch, curve, pick, None);
                    self.under.push(Under::SketchCut { sending, running });
                }
                Queued::SketchSplit { sketch, curve, at } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_split"));
                    let sending = self.sending();
                    let running =
                        sessions.sketch_split(&self.id, self.revision, sketch, curve, at, None);
                    self.under.push(Under::SketchCut { sending, running });
                }
                Queued::SketchExtend {
                    sketch,
                    curve,
                    end,
                    by,
                } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_extend"));
                    let sending = self.sending();
                    let running =
                        sessions.sketch_extend(&self.id, self.revision, sketch, curve, end, by);
                    self.under.push(Under::SketchEdit { sending, running });
                }
                Queued::DimensionValue {
                    serial,
                    sketch,
                    constraint,
                    value,
                } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    let field = format!("constraints.{constraint}.value");
                    self.record(Sent::Edit(field.clone()));
                    let sending = self.sending();
                    let running = sessions.set_field(&self.id, self.revision, sketch, field, value);
                    self.under.push(Under::DimensionValue {
                        serial,
                        sending,
                        running,
                    });
                }
                Queued::Offer { serial, request } => {
                    self.supersede_preview();
                    match request {
                        OfferRequest::SetField {
                            revision,
                            feature,
                            field,
                            value,
                        } => {
                            self.record(Sent::Edit(field.clone()));
                            let sending = Sending {
                                against: revision,
                                state: self.state_of(revision),
                            };
                            let running =
                                sessions.set_field(&self.id, revision, feature, field, value);
                            self.under.push(Under::OfferField { sending, running });
                        }
                        OfferRequest::Undo { revision, to } => {
                            self.record(Sent::Undo(to));
                            self.under.push(Under::Step {
                                running: sessions.undo(&self.id, revision, to),
                                undo: true,
                                sketch: Some(serial),
                                offered: true,
                            });
                        }
                    }
                }
                Queued::Load {
                    serial,
                    kind,
                    feature,
                } => {
                    // Not if the dialog was cancelled while it waited.
                    if self.loading == Some(serial) {
                        let running = sessions.text(&self.id, Some(feature.clone()));
                        self.under.push(Under::Load {
                            serial,
                            kind,
                            feature,
                            running,
                        });
                    }
                }
                Queued::Restore { revision } => {
                    let running = sessions.text_at(&self.id, None, Some(Named::Revision(revision)));
                    self.under.push(Under::Restore {
                        revision,
                        sent_at: self.revision,
                        running,
                    });
                }
                Queued::Replace { revision, text } => {
                    let began = !self.transaction && !self.beginning;
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Replace);
                    let sending = self.sending();
                    self.under.push(Under::Replace {
                        revision,
                        began,
                        sending,
                        running: sessions.replace_text(&self.id, self.revision, text),
                    });
                }
                Queued::Closed { ok, sketch } => {
                    // Every edit sent before it has answered: what a Cancel put back of the
                    // selection is read from the revision its undo or rollback comes to,
                    // past this one, or, if nothing of the dialog's changed the document,
                    // from this one.
                    let undone = self.revision.next().unwrap_or(self.revision);
                    let Some(base) = self.dialog_base.take() else {
                        self.read_restored_from(ok, self.revision);
                        continue;
                    };
                    if !self.transaction {
                        // Nothing of the dialog's changed the document.
                        self.read_restored_from(ok, self.revision);
                        continue;
                    }
                    let unchanged = self.state_of(base) == self.state_of(self.revision);
                    let rolls_back = (!ok || unchanged) && self.dialog_began;
                    if sketch && self.sketch_edited && !rolls_back {
                        // What sketch mode undid, or its Cancel discards, stays so: Redo is
                        // off at the state it leaves (Finish's end, or the state Cancel
                        // undoes back to) till the next edit. Not after a sketch mode that
                        // changed nothing: a redo from before it is the person's still.
                        let at = if ok { self.revision } else { base };
                        self.redo_fence = Some(self.state_of(at));
                    }
                    if rolls_back {
                        self.read_restored_from(ok, undone);
                        // Its own edits began the transaction: nothing before it to keep, and
                        // no transaction left open with nothing in it (an outside change is
                        // then reloaded, section 4).
                        self.supersede_preview();
                        self.record(Sent::Rollback);
                        self.under.push(Under::Rollback {
                            running: sessions.rollback(&self.id),
                            then_close: false,
                        });
                    } else if unchanged {
                        // Nothing of the dialog's changed the document.
                        self.read_restored_from(ok, self.revision);
                    } else if ok {
                        self.groups.push(Group {
                            start: self.state_of(base),
                            end: self.state_of(self.revision),
                        });
                    } else {
                        self.read_restored_from(ok, undone);
                        self.supersede_preview();
                        self.record(Sent::Undo(Some(base)));
                        self.under.push(Under::Step {
                            running: sessions.undo(&self.id, self.revision, Some(base)),
                            sketch: None,
                            undo: true,
                            offered: false,
                        });
                    }
                }
                Queued::Save { then_close } if !self.transaction && !self.beginning => {
                    if then_close {
                        self.under.push(Under::Close(sessions.close(&self.id)));
                        self.edits.queue.clear();
                    }
                }
                Queued::Revert { then_close } if !self.transaction && !self.beginning => {
                    if then_close {
                        self.under.push(Under::Close(sessions.close(&self.id)));
                        self.edits.queue.clear();
                    }
                }
                Queued::Revert { then_close } => {
                    self.supersede_preview();
                    self.record(Sent::Rollback);
                    self.under.push(Under::Rollback {
                        running: sessions.rollback(&self.id),
                        then_close,
                    });
                }
                Queued::SaveAs {
                    path,
                    replace,
                    then_close,
                } => {
                    // A transaction begun first if none is open: a Save As commits one,
                    // and one with nothing changed writes the document all the same.
                    let began = !self.transaction && !self.beginning;
                    if began {
                        self.beginning = true;
                        self.under.push(Under::Begin(sessions.begin(&self.id)));
                    }
                    self.saves += 1;
                    self.under.push(Under::SaveAs {
                        running: sessions.save_as(&self.id, path.clone(), replace),
                        path,
                        began,
                        then_close,
                    });
                }
                Queued::Save { then_close } => {
                    self.saves += 1;
                    self.record(Sent::Commit);
                    self.under.push(Under::Commit {
                        running: sessions.commit(&self.id),
                        then_close,
                    });
                }
                Queued::Step {
                    undo,
                    floor,
                    sketch,
                } => {
                    let state = self.state_of(self.revision);
                    // Sketch mode's undo, come up with the session at its floor already (one
                    // queued behind another that got there): dropped, never past it.
                    if undo && floor.is_some_and(|floor| self.state_of(floor) == state) {
                        continue;
                    }
                    // At a dialog's OK'd edits' end (undo) or start (redo): all of them at
                    // once, to its other side (section 9). Never in sketch mode, whose steps
                    // are its own edits', one at a time.
                    let to = match sketch {
                        Some(_) => None,
                        None => self.groups.iter().find_map(|group| match undo {
                            true => (group.end == state).then_some(group.start),
                            false => (group.start == state).then_some(group.end),
                        }),
                    };
                    self.supersede_preview();
                    let running = if undo {
                        self.record(Sent::Undo(to));
                        sessions.undo(&self.id, self.revision, to)
                    } else {
                        self.record(Sent::Redo(to));
                        sessions.redo(&self.id, self.revision, to)
                    };
                    self.under.push(Under::Step {
                        running,
                        undo,
                        sketch,
                        offered: false,
                    });
                }
            }
        }
    }

    /// The selection a dialog's Cancel put back ([`close_dialog`](Self::close_dialog)) read
    /// once a model of `from` or a later one is published, as that Cancel, `ok` false, is
    /// sent; unless another Cancel waits behind it, whose undo comes later still (and which
    /// says when, as it's sent).
    fn read_restored_from(&mut self, ok: bool, from: Revision) {
        let another = self
            .edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Closed { ok: false, .. }));
        if !ok && !another {
            self.selection.read_from(from);
        }
    }

    /// Whether what's queued waits: an edit, a save, a revert, a step, or a dialog's add (or
    /// its body's read) running.
    fn blocked(&self) -> bool {
        // Reload's check out: what's asked after it waits for it, so an edit's begin never
        // cancels it before it has read the file.
        self.edits.in_flight.is_some()
            || self.reload_out
            || self.dragging()
            || self.under.iter().any(|under| {
                matches!(
                    under,
                    Under::Commit { .. }
                        | Under::SaveAs { .. }
                        | Under::Rollback { .. }
                        | Under::Step { .. }
                        | Under::Move { .. }
                        | Under::Suppress { .. }
                        | Under::Remove { .. }
                        | Under::Add { .. }
                        | Under::Base { .. }
                        | Under::AddSketch { .. }
                        | Under::SketchEdit { .. }
                        | Under::SketchJoin { .. }
                        | Under::SketchAdd { .. }
                        | Under::SketchCut { .. }
                        | Under::DimensionValue { .. }
                        | Under::OfferField { .. }
                        | Under::Restore { .. }
                        | Under::Replace { .. }
                )
            })
    }

    /// Whether sketch mode's drag has anything under way with the core: its begin, moves,
    /// or end. What's queued waits for it, as an edit landing would end it.
    fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(Drag::in_flight)
    }

    /// Sketch mode leaving (Finish, Cancel, a revert, or Create Sketch's add refused), with a
    /// drag: one whose press is still held ended from its begin state (`drag_end` with none,
    /// once its begin answers); one whose begin never went out let go; one released kept, so
    /// its edit lands, and what's queued after it (the leaving's close) waits for its end.
    /// Its end is sent as the drag is next driven (each poll).
    fn leave_drag(&mut self) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !drag.begun_sent() {
            self.drag = None;
            return;
        }
        drag.cancel();
    }

    /// Esc, or a cancel of the press, with a drag under way: ended from its begin state
    /// (`drag_end` with none, once its begin has answered; nothing, if it never began); a
    /// press that never moved let go, its release clicking nothing. Whether one was held.
    fn cancel_drag<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        let Some(drag) = self.drag.as_mut().filter(|drag| drag.held()) else {
            return false;
        };
        if let Some(mode) = self.sketch.as_mut() {
            mode.let_go_press();
        }
        if !drag.moved {
            self.drag = None;
            return true;
        }
        drag.cancel();
        self.drive_drag(sessions);
        true
    }

    /// Whether nothing of the tab's is under way: no edit in flight or queued, nothing
    /// queued, no command running. A revision that moves then isn't the tab's.
    fn quiet(&self) -> bool {
        // A Reload waiting for the tab's check to answer is no change of the tab's: a reload
        // meanwhile is seen as the session's.
        self.edits.in_flight.is_none()
            && self
                .edits
                .queue
                .iter()
                .all(|queued| matches!(queued, Queued::Reload))
            && self.under.is_empty()
            && !self.beginning
            && !self.dragging()
    }

    /// Whether anything of the tab's is under way: an edit, a save, a revert, a step, a
    /// dialog's add or its feature's read, or a marker of a dialog's opening or closing,
    /// queued or running.
    pub fn unsettled(&self) -> bool {
        !self.quiet()
    }

    /// Whether the property panel's field keeps what was typed whatever the document's
    /// name shows: an edit, a save, a revert, or a step still to answer, or the timeline
    /// not yet read of the revision they came to, so a name the document passes through on
    /// the way (an undo's, before the edit typed after it) isn't a reason to show it.
    pub fn applying(&self) -> bool {
        !self.quiet()
            || self
                .timeline
                .as_ref()
                .is_none_or(|timeline| timeline.revision < self.revision)
    }

    /// Whether it's closing: a save, a Save As, or a revert that closes it, or its close,
    /// queued or running. The property panel takes no edit then: it would be dropped.
    pub fn closing(&self) -> bool {
        self.edits.queue.iter().any(|queued| {
            matches!(
                queued,
                Queued::Save { then_close: true }
                    | Queued::SaveAs {
                        then_close: true,
                        ..
                    }
                    | Queued::Revert { then_close: true }
            )
        }) || self.under.iter().any(|under| {
            matches!(
                under,
                Under::Commit {
                    then_close: true,
                    ..
                } | Under::SaveAs {
                    then_close: true,
                    ..
                } | Under::Rollback {
                    then_close: true,
                    ..
                } | Under::Close(_)
            )
        })
    }

    /// `edit` made by the person: inside the tab's transaction, begun first if none is open
    /// (decision 1), and sent now if no edit is in flight, or else waiting (section 3).
    pub fn edit<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, edit: Edit) {
        self.hush();
        self.edits.push(edit);
        self.advance(sessions);
    }

    /// `feature` moved along the timeline before `before`, or to the end for none
    /// (`move_feature`), as an edit is: inside the tab's transaction, begun first if none is
    /// open, and sent once everything asked before it has answered. Never while a dialog or
    /// sketch mode is open (its Cancel undoes to where it opened) or the tab closes: then
    /// it's said why, and nothing is sent. A refusal is said in the status bar.
    pub fn move_feature<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        feature: FeatureId,
        before: Option<FeatureId>,
    ) {
        if self.in_dialog() {
            self.message = Some("Close the dialog before moving a feature".to_owned());
            return;
        }
        // Sketch mode's Cancel undoes to where it opened, and its Finish is one step for
        // undo: a move made inside would be dropped unasked, or folded into the sketch's.
        if self.in_sketch() {
            self.message = Some("Finish or cancel the sketch before moving a feature".to_owned());
            return;
        }
        if self.closing() {
            return;
        }
        self.hush();
        self.edits.queue.push_back(Queued::Move { feature, before });
        self.advance(sessions);
    }

    /// The feature `feature` suppressed, or unsuppressed for `suppressed` false
    /// (`suppress_feature`), as a move is: inside the tab's transaction, so one undo takes it
    /// back, and sent once everything asked before it has answered; never while a dialog is
    /// open or the tab closes (the menus' item is off then). What it suppressed or
    /// unsuppressed with it is said in the status bar, a refusal too.
    pub fn suppress_feature<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        feature: FeatureId,
        suppressed: bool,
    ) {
        if self.in_dialog() {
            self.message = Some("Close the dialog before suppressing a feature".to_owned());
            return;
        }
        if self.closing() {
            return;
        }
        self.hush();
        self.edits.queue.push_back(Queued::Suppress {
            feature,
            suppressed,
        });
        self.advance(sessions);
    }

    /// The feature `feature` deleted (`remove_feature`; the prototype's Delete, which asks
    /// nothing), as a move is: one undo puts it back. What consumed it takes its base, which
    /// the status bar lists; where nothing can stand in, the core refuses, naming both, and
    /// that's said. Never while a dialog is open or the tab closes.
    pub fn remove_feature<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        feature: FeatureId,
    ) {
        if self.in_dialog() {
            self.message = Some("Close the dialog before deleting a feature".to_owned());
            return;
        }
        if self.closing() || self.removing(&feature) {
            // A second Del before the first answers: it's going already.
            return;
        }
        self.hush();
        self.edits.queue.push_back(Queued::Remove { feature });
        self.advance(sessions);
    }

    /// Whether a deletion of `feature` is queued or running.
    fn removing(&self, feature: &FeatureId) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Remove { feature: queued } if queued == feature))
            || self.under.iter().any(
                |under| matches!(under, Under::Remove { feature: under, .. } if under == feature),
            )
    }

    /// The feature `feature` renamed `name` (`set_field` of `name`), as the property panel's
    /// Name field renames it: an edit, coalesced with one of the same field waiting. A name
    /// that doesn't read as a string is never sent.
    pub fn rename<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        feature: FeatureId,
        name: &str,
    ) {
        if let Ok(value) = name_value(name) {
            self.edit(
                sessions,
                Edit {
                    feature,
                    field: "name".to_owned(),
                    value,
                },
            );
        }
    }

    /// The name typed in the feature `feature`'s own field, finished (Enter, or a click
    /// away): sent trimmed, unless that leaves nothing (dropped, as Esc drops it) or the
    /// name it has, or the timeline doesn't list it. Whether it was sent.
    pub fn finish_rename<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        feature: FeatureId,
        typed: &str,
    ) -> bool {
        let Some(name) = crate::feature_menu::finished(typed) else {
            return false;
        };
        if self
            .entry(feature.as_str())
            .is_none_or(|entry| entry.name == name)
        {
            return false;
        }
        self.rename(sessions, feature, name);
        true
    }

    /// The feature `feature`'s name typed in its own field, at `place`: its timeline item's
    /// (the timeline scrolled to it, so the field over it is in view) or, if it has one, its
    /// browser row's (the browser scrolled to it, its group opened; else its item's). Never while a dialog is open or the tab closes, nor
    /// for a feature the timeline doesn't list.
    pub fn start_rename(&mut self, feature: FeatureId, place: crate::feature_menu::Place) {
        if self.in_dialog() || self.closing() {
            return;
        }
        let Some(entry) = self.entry(feature.as_str()) else {
            return;
        };
        let text = entry.name.clone();
        let place = match place {
            crate::feature_menu::Place::Browser if !self.in_browser(&feature) => {
                crate::feature_menu::Place::Timeline
            }
            place => place,
        };
        match place {
            crate::feature_menu::Place::Timeline => {
                self.selection.select_feature(feature.clone());
                self.timeline_scroll = Some(feature.clone());
            }
            crate::feature_menu::Place::Browser => {
                self.selection.find_in_browser(feature.clone());
            }
        }
        self.renaming = Some(crate::feature_menu::Renaming::new(feature, place, text));
    }

    /// Whether the browser has a row for the feature `id`: a body's or a sketch's.
    pub fn in_browser(&self, id: &FeatureId) -> bool {
        self.browser
            .bodies
            .iter()
            .chain(&self.browser.sketches)
            .any(|row| &row.id == id)
    }

    /// The feature `id`'s name, as the timeline last listed it: its ID if it doesn't.
    pub(crate) fn name_of(&self, id: &FeatureId) -> String {
        self.entry(id.as_str())
            .map_or_else(|| id.as_str().to_owned(), |entry| entry.name.clone())
    }

    /// The person's next action: the last refusal said, and what the last suppression or
    /// deletion did, let go.
    fn hush(&mut self) {
        self.message = None;
        self.told = None;
    }

    /// Save (`commit`), then, if `then_close`, close: once everything asked before it has
    /// answered, so the commit saves every edit made before it. One queued last already takes
    /// `then_close` too. A save that comes up with nothing to save does nothing (or closes).
    pub fn save<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, then_close: bool) {
        self.hush();
        match self.edits.queue.back_mut() {
            Some(Queued::Save { then_close: last }) => *last |= then_close,
            _ => self.edits.queue.push_back(Queued::Save { then_close }),
        }
        self.advance(sessions);
    }

    /// Revert (`rollback`): the file's text, byte for byte; then, if `then_close` or a
    /// revert it drops was to close the tab, close.
    /// The edits and steps queued since the last save queued are dropped, and the revert
    /// waits behind that save, which saves what was asked before it (#298's review): one in
    /// flight answers first, and the rollback undoes it. Also how a file changed on disk is
    /// reloaded with edits held: the session reloads it once the transaction has ended
    /// (section 4).
    pub fn revert<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, then_close: bool) {
        self.hush();
        // The session reloads the file changed on disk once the edits are gone: at the
        // person's asking.
        if self
            .status
            .as_ref()
            .is_some_and(|status| matches!(status.disk, Some(Disk::Changed | Disk::Gone)))
        {
            self.reload_asked = true;
        }
        // The document goes back to the file's: a dialog or a sketch open on it has nothing
        // to edit.
        self.dialog = None;
        self.loading = None;
        self.leave_drag();
        self.sketch = None;
        self.drive_drag(sessions);
        let kept = self
            .edits
            .queue
            .iter()
            .rposition(|queued| matches!(queued, Queued::Save { .. } | Queued::SaveAs { .. }))
            .map_or(0, |save| save + 1);
        // A revert queued to close it (Don't save) still closes it: its intent is carried.
        let then_close = then_close
            || self
                .edits
                .queue
                .range(kept..)
                .any(|queued| matches!(queued, Queued::Revert { then_close: true }));
        self.edits.queue.truncate(kept);
        self.edits.queue.push_back(Queued::Revert { then_close });
        self.advance(sessions);
    }

    /// Close, its edits saved or dropped first (`close` is refused inside a transaction,
    /// decision 22); any waiting dropped.
    pub fn close<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        // A transaction open with nothing changed (undone back to what `begin` saw): rolled
        // back, then closed, as `close` is refused inside one, losing nothing.
        if self.transaction || self.beginning {
            self.revert(sessions, true);
            return;
        }
        self.edits.queue.clear();
        self.under.push(Under::Close(sessions.close(&self.id)));
    }

    /// Undo or redo one edit of the transaction, once the edits made before it have
    /// answered.
    pub fn step<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, undo: bool) {
        self.hush();
        // Sketch mode's own steps go no further back than where it opened.
        let (floor, sketch) = match &self.sketch {
            Some(mode) => (mode.undo_floor.or(mode.opened_at), Some(mode.serial)),
            None => (None, None),
        };
        self.edits.queue.push_back(Queued::Step {
            undo,
            floor,
            sketch,
        });
        self.advance(sessions);
    }

    /// `export` of the session's current model, as `request` asks (M5.12), off the UI thread
    /// as every command is, unless one is running. What the last one said is let go.
    pub fn export<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, request: Export) {
        if self.exporting.is_some() {
            return;
        }
        self.export_said = None;
        self.export_replacing = None;
        self.export_answer = None;
        self.exported = false;
        self.exports.push(request.clone());
        // One at a time, so never more than one past.
        if self.exports.len() > SENT_KEPT {
            self.exports.remove(0);
        }
        let running = sessions.export(&self.id, request.clone());
        self.exporting = Some((request, running));
    }

    /// The export whose path held a file sent again, to replace it, as the person said.
    pub fn replace_export<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(mut request) = self.export_replacing.take() {
            request.replace = true;
            self.export(sessions, request);
        }
    }

    /// The export dialog's Cancel: the export stopped through its handle (it answers
    /// `cancelled`, writing nothing), and a question about a file at its path let go.
    pub fn stop_export(&mut self) {
        self.export_replacing = None;
        // Through its own handle: queued behind another command or running, it's this
        // export stopped, never another client's; one that has answered has nothing left.
        if let Some((_, running)) = &self.exporting {
            running.cancel();
        }
    }

    /// The export dialog opened: what the last one left (its refusal, its question) let
    /// go, as the person's next action lets a refusal go.
    pub fn export_opened(&mut self) {
        self.export_said = None;
        self.export_replacing = None;
        self.hush();
        // An export still out (its dialog cancelled, its answer not in yet) isn't this
        // dialog's: its answer neither closes it nor asks it anything.
        self.export_disowned = self.exporting.is_some();
    }

    /// Whether an export is running.
    pub fn exporting(&self) -> bool {
        self.exporting.is_some()
    }

    /// Whether the last export wrote its file since this was last asked.
    pub fn take_exported(&mut self) -> bool {
        std::mem::take(&mut self.exported)
    }

    /// The export running polled: its answer kept; a file at its path, not to be replaced,
    /// to ask about; any other refusal said, in the dialog and as every command's is (a
    /// cancelled one, which the person asked for, said nowhere).
    fn take_export(&mut self) {
        let Some((asked, running)) = self.exporting.take() else {
            return;
        };
        match running.try_take() {
            Ok(response) => {
                let disowned = std::mem::take(&mut self.export_disowned);
                match &response.result {
                    _ if disowned => {}
                    Ok(_) => self.exported = true,
                    Err(problems) => {
                        let first = problems.errors().first();
                        if first.and_then(|error| error.suggestion())
                            == Some(silico_core::file::REPLACE_ADVICE)
                        {
                            self.export_replacing = Some(asked);
                        } else if first.is_none_or(|error| error.code() != ErrorCode::Cancelled) {
                            let said = said(problems);
                            self.message = Some(said.clone());
                            self.export_said = Some(said);
                        }
                    }
                }
                self.export_answer = Some(response.result);
            }
            Err(running) => self.exporting = Some((asked, running)),
        }
    }

    /// Whether the stop is on: the session's rebuild running, as `status` says, whatever
    /// command runs (section 3, *Stop*; decision 34 as amended). A command running with no
    /// rebuild (an export meshing) is its own dialog's to stop, and the session's own
    /// reload nothing's.
    pub fn stoppable(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.rebuild.state == RebuildState::Running)
    }

    /// The stop: `cancel` naming the rebuild, which stops it and the command waiting on it,
    /// if one is (that one answers `cancelled`), the tab's own read of the model stopped
    /// with it ([`halt`](Self::halt)). One that comes after the rebuild ended is refused,
    /// and the refusal is said.
    pub fn stop<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        match self.halt(sessions) {
            Ok(()) => self.message = None,
            Err(problems) => self.message = Some(said(&problems)),
        }
    }

    /// The rebuild stopped, as `cancel` naming `rebuild` stops it, and the tab's own reads
    /// of the model with it, each cancelled through its handle first: its `timeline` read
    /// (a preview's, or Rebuild's) and its `check` read, of the current model while nothing
    /// is published. One still queued behind another command when the rebuild stops isn't
    /// its waiter yet: run after the stop, it would need the model and start the rebuild
    /// again, as the next command that needs it does (decision 34), undoing the stop the
    /// person asked for. Cancelled first, it answers `cancelled` whenever it runs, starting
    /// nothing, so once this returns nothing of the tab's starts the stopped rebuild again
    /// (the tab then reads the published model, section 3, and asks no check of the current
    /// model while the rebuild is stopped: [`read_check`](Self::read_check)). A check
    /// cancelled is asked again once it may be. A read already waiting, cancelled, stops the
    /// rebuild itself, and may do so before the cancel naming the rebuild reaches it: that
    /// cancel's refusal is then the stop's success, as `status` says the rebuild stopped.
    /// Only a cancel the stop makes counts so: a read an edit superseded already
    /// ([`supersede_preview`](Self::supersede_preview)) was cancelled by the edit, which
    /// stops its rebuild, not by the stop (a cancel of a cancelled read says it stopped it
    /// all the same). Refused with the cancel's refusal only if the rebuild wasn't stopped:
    /// it ended (published, failed, or superseded) before the stop reached it.
    ///
    /// The two reads are cancelled the one sent later first
    /// ([`cancel_own_reads`](Self::cancel_own_reads)).
    fn halt<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> Result<(), Problems> {
        self.halt_with(sessions, |_| ())
    }

    /// [`halt`](Self::halt), with `between` called after the first of the tab's reads is
    /// cancelled and before the second is, told which was: where a test holds the stop to
    /// let the session run what that cancel lets it.
    fn halt_with<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        between: impl FnOnce(OwnRead),
    ) -> Result<(), Problems> {
        let (timeline_stopped, check_stopped) = self.cancel_own_reads(between);
        if check_stopped {
            self.checked = None;
        }
        let read_stopped = timeline_stopped || check_stopped;
        let refused = match sessions.cancel(&self.id, Cancellable::Rebuild).result {
            Ok(_) => return Ok(()),
            Err(problems) => problems,
        };
        let stopped = read_stopped
            && sessions.status(&self.id).result.is_ok_and(|status| {
                status.rebuild.revision == self.revision
                    && status.rebuild.state == RebuildState::Stopped
            });
        if stopped { Ok(()) } else { Err(refused) }
    }

    /// The tab's timeline and check reads out cancelled through their handles, the one sent
    /// later first ([`check_first`](Self::check_first)), `between` called after the first
    /// and before the second, told which was (where a test holds it to let the session run
    /// what that cancel lets it): whether each, timeline then check, was stopped before it
    /// had done its work. The session runs its queued commands in the order sent, and the
    /// earlier read, the rebuild's waiter, cancelled, stops the rebuild and answers at once,
    /// its session going straight on to the next in its queue. Cancelled second, the later
    /// read could be run in between, uncancelled, and start the stopped rebuild again
    /// (decision 34); cancelled first, it never runs. A read an edit superseded was
    /// cancelled by that edit already, so it's left, counted as not stopped by this.
    fn cancel_own_reads(&self, between: impl FnOnce(OwnRead)) -> (bool, bool) {
        let [first, second] = if self.check_first {
            [OwnRead::Timeline, OwnRead::Check]
        } else {
            [OwnRead::Check, OwnRead::Timeline]
        };
        let first_stopped = self.cancel_own(first);
        between(first);
        let second_stopped = self.cancel_own(second);
        if first == OwnRead::Timeline {
            (first_stopped, second_stopped)
        } else {
            (second_stopped, first_stopped)
        }
    }

    /// The tab's read `read` cancelled through its handle, if it's out and no edit
    /// superseded it: whether it was stopped before it had done its work.
    fn cancel_own(&self, read: OwnRead) -> bool {
        match read {
            OwnRead::Timeline => {
                !self.timeline_superseded
                    && self
                        .timeline_read
                        .as_ref()
                        .is_some_and(|read| read.cancel())
            }
            OwnRead::Check => {
                !self.check_superseded
                    && self
                        .check_read
                        .as_ref()
                        .is_some_and(|(_, read)| read.cancel())
            }
        }
    }

    /// Rebuild (Ctrl+B, docs/ux/solidworks.md, §5, *Rebuild*): the tab's revision's rebuild,
    /// stopped, started again by a read of the current model (from the cache, so what was
    /// built before the stop isn't built again), whose answer is the timeline. Only while
    /// that rebuild is stopped.
    pub fn rebuild<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if !self.can_rebuild() {
            return;
        }
        // A read of the published model in flight answers at once: this one replaces it.
        self.timeline_asked = Some((self.revision, self.published));
        self.check_first = self.check_read.is_some();
        self.timeline_read = Some(sessions.timeline(&self.id, ModelChoice::Current));
        self.timeline_superseded = false;
        self.rebuild_out = true;
        self.rebuilds += 1;
    }

    /// Rebuild everything (Shift+Ctrl+B; DESIGN.md, *Keyboard*: "every feature, ignoring
    /// the cache"): `check` of the current model with `fresh`, which rebuilds the tab's
    /// revision with every step built anew, reading nothing kept, neither the session's
    /// steps nor the cache on disk, as SolidWorks' forced rebuild (Ctrl+Q) does, a fault
    /// kept as the revision's outcome tried again. A stopped rebuild is started so too.
    /// Read as the tab's own check is, so the stop (Esc, Stop) stops it as it stops that
    /// one, and an edit supersedes it; its problems are what the property panel says. Only
    /// while [`can_rebuild_all`](Self::can_rebuild_all).
    pub fn rebuild_all<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if !self.can_rebuild_all() {
            return;
        }
        let gate = self
            .timeline
            .as_ref()
            .is_some_and(|timeline| timeline.problems > 0);
        // Sent after any timeline read out: the later of the two, cancelled first.
        self.check_first = false;
        self.check_superseded = false;
        self.fresh_out = true;
        self.check_read = Some((
            gate,
            sessions.check(&self.id, CheckModel::Current { fresh: true }),
        ));
        self.rebuilds_all += 1;
    }

    /// Whether the session last found its file gone from disk (`status.disk`).
    pub fn file_gone(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.disk == Some(Disk::Gone))
    }

    /// Reload (Trevin's ask of 2026-11-09; ⌘R on macOS, Ctrl+R elsewhere, and File >
    /// Reload): the part's file read from disk again now, then every feature rebuilt, as
    /// Rebuild everything rebuilds them; the edits held dropped first, as Revert drops them
    /// (the app asks before, naming them). Outside a transaction every command reads the file
    /// first, so Rebuild everything's `check` is the read: a file changed on disk is reloaded
    /// by it, before the disk watch's next comparison, and that is what's rebuilt; the
    /// reload is the person's, so no banner says another program's. The check is queued
    /// behind the revert ([`Queued::Reload`]), so an edit asked after it comes after it.
    ///
    /// An untitled part has no file: it's only rebuilt, its edits kept. On a file gone from
    /// disk nothing is reloaded or dropped: the part open here is the only copy of the work,
    /// so that's said, with Save As to keep it (Main's ruling of 2026-11-09; the core refuses
    /// a read of a gone file outside a transaction, so nothing can be rebuilt). Nothing with
    /// a dialog open, as Rebuild everything does nothing then (the app says why), while the
    /// tab closes, or before the document is read.
    pub fn reload<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if self.closing() || self.timeline.is_none() || self.in_dialog() {
            return;
        }
        if self.file_gone() {
            self.toast = Some(format!(
                "{} is gone from disk: nothing to reload. Save As keeps your work",
                self.name()
            ));
            return;
        }
        // An untitled part has no file to read and its edits are all it is: nothing reverted,
        // its rebuild queued as a titled part's is.
        if !self.is_untitled() {
            self.reload_asked = true;
            if self.unsaved() || self.transaction {
                self.revert(sessions, false);
            }
        }
        self.edits.queue.push_back(Queued::Reload);
        self.advance(sessions);
    }

    /// Whether Reload's Rebuild everything waits in the queue.
    fn reload_queued(&self) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Reload))
    }

    /// Whether Rebuild everything may be sent: the document read, no dialog open (its
    /// preview is its own rebuild), not closing, and no check of the tab's out, which a
    /// second would only queue behind.
    pub fn can_rebuild_all(&self) -> bool {
        self.timeline.is_some() && !self.in_dialog() && !self.closing() && self.check_read.is_none()
    }

    /// Whether Rebuild is on: the tab's revision's rebuild stopped, and no read of the
    /// current model of the tab's own already out (one Rebuild sent, not yet started by the
    /// session, which a second would only queue behind).
    pub fn can_rebuild(&self) -> bool {
        self.rebuild_stopped() && !self.rebuild_out
    }

    /// Esc with no dialog open (section 3's order, the prototype's `escChain`, the nearest
    /// thing first): sketch mode's tool left, its chain or gesture ended with it (section 9,
    /// *Line*); or else sketch mode's selection cleared, if it has one; or else the reload's
    /// purple banner dismissed, if one shows (DESIGN.md, 6c; an amber or red one in its
    /// place has let it go already); or else the feature's selection cleared; or else, with
    /// none of them, a rebuild running stopped quietly
    /// ([`stop_quietly`](Self::stop_quietly)).
    pub fn escape<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        // A drag held ends from its begin state: section 3's first, before anything else.
        if self.cancel_drag(sessions) {
            return;
        }
        // The canvas's right-click menu closes.
        if self
            .sketch
            .as_mut()
            .is_some_and(|mode| mode.menu_at.take().is_some())
        {
            return;
        }
        // The over-define prompt's Esc is its Cancel (the prototype's key).
        if self
            .sketch
            .as_ref()
            .is_some_and(|mode| mode.offer().is_some())
        {
            self.sketch_offer(sessions, OfferChoice::Cancel);
            return;
        }
        // A placed dimension's Modify box closes, its value as it was.
        if self
            .sketch
            .as_mut()
            .is_some_and(|mode| mode.edit_box.take().is_some())
        {
            return;
        }
        let tool_left = self.sketch.as_mut().is_some_and(SketchMode::escape_tool);
        if tool_left {
            self.sketch_requests(sessions);
            return;
        }
        if let Some(mode) = self
            .sketch
            .as_mut()
            .filter(|mode| !mode.selection.is_empty())
        {
            mode.selection.clear();
        } else if self.reloaded.is_some() {
            self.reloaded = None;
        } else if !self.selection.is_empty() {
            self.selection.clear();
        } else if self.stoppable() {
            self.stop_quietly(sessions);
        }
    }

    /// The stop, from Esc or a dialog preview: as the button's ([`stop`](Self::stop)), but
    /// a refusal (the rebuild finished since `status` said it ran, leaving nothing to stop)
    /// says nothing. Only the Stop button says its refusal. Whether it stopped the rebuild.
    pub fn stop_quietly<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        let stopped = self.halt(sessions).is_ok();
        if stopped {
            self.hush();
        }
        stopped
    }

    /// Esc with a dialog open (section 3's order):
    /// the preview's rebuild, if it's rebuilding, stopped, the dialog left open, as
    /// SolidWorks stops a slow preview (docs/ux/solidworks.md, §5); or else the dialog's
    /// Cancel, as Esc is in SolidWorks' PropertyManager. `status`, read at the frame's start,
    /// may say the preview rebuilds when its rebuild has ended since: the stop then finds
    /// nothing to stop, and the Esc is the Cancel, never eaten. So it is once an edit sent
    /// since (in the same frame, before the Esc) superseded the preview's read: the edit
    /// stops that rebuild, of a revision it makes old, and the next revision's preview
    /// hasn't begun, so there's no preview of the Esc's to stop. Whether it's the Cancel.
    pub fn dialog_escape<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        !(self.preview_rebuilding() && !self.timeline_superseded && self.stop_quietly(sessions))
    }

    /// The choice section 4 offers once the file changed on disk while unsaved edits are
    /// held.
    fn offer_disk_choice(&mut self) {
        // A status read before a reload the tab saw since, in an answer taken after it in
        // the same frame (Restore's `begin`, which read the file, answering as the frame
        // read the status), compared the file with the bytes that reload replaced: what it
        // found says nothing of the file now, and offers no choice over the reload's banner.
        let stale = self
            .reloaded_to
            .is_some_and(|reloaded_to| self.status_revision < reloaded_to);
        let disk = self
            .status
            .as_ref()
            .and_then(|status| status.disk)
            .filter(|_| !stale);
        let changed = matches!(disk, Some(Disk::Changed | Disk::Gone));
        self.disk_choice = match (self.disk_choice, changed, self.unsaved(), disk) {
            (_, false, _, _) | (_, _, false, _) => DiskChoice::None,
            (DiskChoice::Kept, true, true, _) => DiskChoice::Kept,
            (_, true, true, Some(disk)) => DiskChoice::Offered(disk),
            (choice, _, _, None) => choice,
        };
    }

    /// The timeline read again once the session's revision or the published model's
    /// changes (section 3, *When the app reads*): of the published model, or, while nothing
    /// is published, of the current one, whose read waits for its rebuild.
    fn read_timeline<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let asked = (self.revision, self.published);
        if self.timeline_read.is_some() || self.timeline_asked == Some(asked) {
            return;
        }
        // Stopped with nothing published: a read of the published model is refused for
        // that, and one of the current model would start the rebuild again. Nothing is read
        // until the revision or the published model moves, or Rebuild asks.
        if self.rebuild_stopped() && self.published.is_none() && self.nothing_published {
            return;
        }
        // Never the current model of a revision whose rebuild was stopped: its read would
        // start the rebuild again, undoing the stop. The stopped state shows until an edit.
        let current = ((self.published.is_none() && self.nothing_published) || self.previews())
            && !self.rebuild_stopped();
        let model = if current {
            ModelChoice::Current
        } else {
            ModelChoice::Published
        };
        self.timeline_asked = Some(asked);
        self.check_first = self.check_read.is_some();
        self.timeline_read = Some(sessions.timeline(&self.id, model));
    }

    /// Whether the timeline is read of the current model, as a dialog's preview: a dialog
    /// open on its feature (unless the revision's rebuild was stopped: [`read_timeline`]).
    /// That read waits for the revision's rebuild, so its health is the preview's, and the
    /// stop, which Esc is, stops it (section 3).
    ///
    /// [`read_timeline`]: Self::read_timeline
    fn previews(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
    }

    /// Whether the rebuild of the tab's revision was stopped, as `status` says: Rebuild is
    /// on, and the panes say so where they'd say they're building.
    pub fn rebuild_stopped(&self) -> bool {
        self.status.as_ref().is_some_and(|status| {
            status.rebuild.revision == self.revision
                && status.rebuild.state == RebuildState::Stopped
        })
    }

    /// Whether a dialog's preview is rebuilding: a dialog open on its feature (one that
    /// isn't added yet has no preview) while the session's rebuild runs. Esc stops it, the
    /// dialog left open (section 3), with the stop's own call ([`stop`](Self::stop)): the
    /// preview's read of the current model is the rebuild's waiter, and answers `cancelled`.
    pub fn preview_rebuilding(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.is_some())
            && self.stoppable()
    }

    /// Before the tab sends what changes the document: its timeline and check reads in
    /// flight, of a revision this makes old, cancelled through their handles (queued or
    /// running), the one sent later first ([`cancel_own_reads`](Self::cancel_own_reads)), so
    /// what's sent doesn't wait for a rebuild nothing will show (section 3, *Bursts of
    /// edits*): a preview's read, which waits for the rebuild, and a check of the current
    /// model queued with it, which, left, would start that rebuild again once the read
    /// stopped it (a read of the published model answers at once, so there's nothing of it
    /// to stop). A check stopped is asked again once it may be.
    fn supersede_preview(&mut self) {
        let (_, check_stopped) = self.cancel_own_reads(|_| ());
        if check_stopped {
            self.checked = None;
        }
        self.timeline_superseded |= self.timeline_read.is_some();
        self.check_superseded |= self.check_read.is_some();
    }

    /// A transaction begun if none is open or beginning (decision 1).
    fn begin_if_none<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if !self.transaction && !self.beginning {
            self.beginning = true;
            self.record(Sent::Begin);
            self.under.push(Under::Begin(sessions.begin(&self.id)));
        }
    }

    /// `sent` kept in the call record, the oldest let go past [`SENT_KEPT`].
    fn record(&mut self, sent: Sent) {
        self.sent.push(sent);
        // One at a time, so never more than one past.
        if self.sent.len() > SENT_KEPT {
            self.sent.remove(0);
        }
    }

    /// The state the session is at at `revision`, named by the first revision it was at it:
    /// an undo's or a redo's revision maps to the first of the state it restored.
    fn state_of(&self, revision: Revision) -> Revision {
        self.restored.get(&revision).copied().unwrap_or(revision)
    }

    /// An edit or an add about to be sent, as it's sent: against the tab's revision, at the
    /// state the session is at.
    fn sending(&self) -> Sending {
        Sending {
            against: self.revision,
            state: self.state_of(self.revision),
        }
    }

    /// An edit's or an add's answer, `response`, to what was `sending`: if it was applied
    /// (taken, and the session's revision moved past the one it was sent against), the
    /// dialogs' groups past the state it was made at let go, as it dropped the redo branch
    /// they're on (a group ending after that state was undone to reach it). A refused
    /// change, or one that changed nothing, leaves the redo branch whole, and them with it.
    fn applied<T>(&mut self, sending: Sending, response: &Response<T>) {
        let moved = response
            .session
            .as_ref()
            .is_some_and(|session| session.revision != sending.against);
        if response.result.is_ok() && moved {
            self.groups.retain(|group| group.end <= sending.state);
            self.redo_fence = None;
            self.sketch_edited = true;
        }
    }

    /// The tip of the document as the tab's timeline last listed it ([`tip_of`]): whether a
    /// sweep that cuts has a body to cut, for CREATE's tools. A sweep's add takes its body
    /// from the document as it is when it's sent.
    fn tip(&self) -> Option<FeatureId> {
        tip_of(self.timeline.as_ref()?.features.as_ref()?)
    }

    /// Whether the timeline lists a sketch of ID `id`.
    fn is_sketch(&self, id: &FeatureId) -> bool {
        self.browser.sketches.iter().any(|row| &row.id == id)
    }

    /// Whether `what`'s row shows (its eye open).
    pub fn shown(&self, what: &Hideable) -> bool {
        !self.hidden.contains(what)
    }

    /// `what` hidden if it shows, shown if it's hidden: its eye clicked.
    pub fn toggle_shown(&mut self, what: Hideable) {
        if !self.hidden.remove(&what) {
            self.hidden.insert(what);
        }
    }

    /// H (DESIGN.md, *Keyboard*: Fusion's V, moved to H): the selected feature hidden or
    /// shown, if the browser lists it (a body or a sketch); nothing otherwise.
    pub fn toggle_selected_shown(&mut self) {
        let Some(id) = self.selection.feature().cloned() else {
            return;
        };
        let listed = self
            .browser
            .bodies
            .iter()
            .chain(&self.browser.sketches)
            .any(|row| row.id == id);
        if listed {
            self.toggle_shown(Hideable::Feature(id));
        }
    }

    /// Whether a dialog of `kind` can open: none open or loading, no sketch open, the tab not
    /// closing, the gate taking the document, and, for one that cuts or dresses edges, a body
    /// to cut from or whose edges to dress.
    pub fn can_open(&self, kind: Kind) -> bool {
        self.dialog.is_none()
            && self.loading.is_none()
            && self.sketch.is_none()
            && !self.closing()
            && self
                .timeline
                .as_ref()
                .is_some_and(|timeline| timeline.problems == 0)
            && (!(kind.cuts() || kind.dresses()) || self.tip().is_some())
    }

    /// Whether a new feature's dialog of `kind` can open (its command, on the toolbar): as
    /// any dialog can ([`can_open`](Self::can_open)), and, for a boolean, with two bodies to
    /// combine, as the browser lists them (the bodies it could pick). An existing boolean's
    /// Edit Feature needs none: it consumes the bodies it combines.
    pub fn can_create(&self, kind: Kind) -> bool {
        self.can_open(kind) && (!kind.combines() || self.browser.bodies.len() >= 2)
    }

    /// Why a new feature's dialog of `kind` can't open, if it's for want of what the
    /// document has, said in its command's tooltip, once the timeline is read: a document
    /// the gate (or the reader) refused, as a feature's menu says it ([`UNMENDED`]); else a
    /// boolean with fewer than two bodies.
    pub fn why_closed(&self, kind: Kind) -> Option<&'static str> {
        let timeline = self.timeline.as_ref()?;
        if timeline.problems > 0 {
            return Some(UNMENDED);
        }
        (kind.combines() && self.browser.bodies.len() < 2).then_some(NEEDS_TWO_BODIES)
    }

    /// A new feature's dialog opened (section 9): its inputs at their defaults, or those its
    /// kind was last OK'd with, if it remembers them (`remembered`); a sweep's sketch the one
    /// selected if it's one, and its body the tip. Its feature is added as soon as its
    /// inputs are valid.
    pub fn open_dialog<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        kind: Kind,
        remembered: &crate::dialog::Remembered,
    ) {
        if !self.can_create(kind) {
            return;
        }
        let serial = self.opening();
        let sketch = self
            .selection
            .feature()
            .cloned()
            .filter(|id| self.is_sketch(id));
        let mut dialog = Dialog::new(kind, sketch, serial);
        remembered.apply(&mut dialog);
        if let Some(kinds) = dialog.takes() {
            self.before_dialog = Some(self.selection.saved());
            if dialog.takes_bodies() {
                self.take_bodies(&mut dialog, kinds);
            } else {
                self.take_selection(&mut dialog, kinds);
            }
        }
        self.dialog = Some(dialog);
        self.dialog_changed(sessions);
        self.advance(sessions);
    }

    /// The selection taken into `dialog`'s box of names, which takes `kinds`, as SolidWorks'
    /// PropertyManager box takes what's selected when its command starts: what else is
    /// selected let go and said; the rest's names as a store writes them, or, if one can't
    /// be stored, or they're on several bodies, none, said and the selection cleared (the
    /// canvas shows what the box holds); and the canvas's picks the box's from then.
    fn take_selection(&mut self, dialog: &mut Dialog, kinds: crate::selection::Filter) {
        let left_out = self.selection.keep_only(kinds).words().and_then(|words| {
            dialog.names().map(|names| {
                format!(
                    "{words} left out: {} takes {}",
                    names.of.a_noun(),
                    names.takes.plural()
                )
            })
        });
        let stored = self.selection.storable();
        let mut bodies: Vec<FeatureId> = Vec::new();
        for selected in self.selection.elements().filter_map(|p| p.found()) {
            if !bodies.contains(&selected.feature) {
                bodies.push(selected.feature.clone());
            }
        }
        if stored.is_err() || bodies.len() > 1 {
            self.selection.clear();
        }
        dialog.take_selection(stored, &bodies, left_out);
        self.selection.take_into(Some(kinds));
    }

    /// The selection taken into `dialog`'s boxes of one body, which take `kinds` from the
    /// canvas: the bodies selected (the browser's body rows), in the order selected, the
    /// first its target and the second its tool; what else is selected (a face, an edge, or
    /// a vertex, which fills no body box, as SolidWorks' help doesn't say one does; another
    /// feature; a third body) left out, and said; the selection then the bodies the boxes
    /// hold, and the canvas's picks the boxes' from then.
    fn take_bodies(&mut self, dialog: &mut Dialog, kinds: crate::selection::Filter) {
        let mut bodies: Vec<FeatureId> = Vec::new();
        for item in self.selection.items() {
            if let crate::selection::Item::Feature(id) = item
                && bodies.len() < 2
                && !bodies.contains(id)
                && self.browser.bodies.iter().any(|row| &row.id == id)
            {
                bodies.push(id.clone());
            }
        }
        // Everything else let go and counted, as a box of names counts what it leaves out:
        // every feature but the bodies taken, and every element.
        let none = crate::selection::Filter {
            faces: false,
            edges: false,
            vertices: false,
        };
        let mut left = self.selection.keep_only(none);
        left.features = left.features.saturating_sub(bodies.len());
        let left_out = left.words().map(|words| {
            format!(
                "{words} left out: {} takes two bodies",
                dialog.kind.a_noun()
            )
        });
        dialog.take_bodies(&bodies, left_out);
        self.selection.select_features(dialog.bodies());
        self.selection.take_into(Some(kinds));
    }

    /// A dialog opening: numbered, the revision it opens at marked in the queue, and what
    /// the last one left (a refusal said, its refused fields) let go. Its number.
    fn opening(&mut self) -> u64 {
        self.hush();
        self.refused.clear();
        self.before_dialog = None;
        self.dialogs += 1;
        self.edits.queue.push_back(Queued::Opened {
            serial: self.dialogs,
        });
        self.dialogs
    }

    /// Edit Feature (section 9): the dialog of the feature `id`, its values read from the
    /// document (`text` of it), if a dialog edits its type.
    pub fn edit_feature<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        id: &FeatureId,
    ) {
        let Some(kind) = self.kind_of(id) else {
            return;
        };
        if !self.can_open(kind) {
            return;
        }
        self.selection.select_feature(id.clone());
        self.load(sessions, kind, id.clone());
    }

    /// The dialog of the feature `feature`, of `kind`, opened and read once everything
    /// asked before has answered.
    fn load<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        kind: Kind,
        feature: FeatureId,
    ) {
        let serial = self.opening();
        self.loading = Some(serial);
        self.edits.queue.push_back(Queued::Load {
            serial,
            kind,
            feature,
        });
        self.advance(sessions);
    }

    /// The session reloaded its file with a dialog open: an existing
    /// feature's dialog is read again, opening at the reloaded revision, its values the
    /// file's; a new feature's not yet added opens at the reloaded revision; one already
    /// added can't have been reloaded under (its add opened a transaction, and a session
    /// never reloads inside one), but if it was, it's closed, said.
    fn reloaded_under_dialog<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        // Sketch mode on a sketch with nothing of its own sent (a session reloads only with
        // no transaction open, and Create Sketch's add opens one): it opens at the reloaded
        // revision, its next read the reloaded sketch's.
        if let Some(mode) = self.sketch.as_mut() {
            self.dialog_base = Some(self.revision);
            self.dialog_began = true;
            mode.opened_at = Some(self.revision);
            // What its tool added is the file's no longer: forgotten, with its gesture.
            mode.tools.forget_pending();
        }
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        match (dialog.made, dialog.feature.clone()) {
            (false, Some(feature)) => self.load(sessions, dialog.kind, feature),
            (true, None) => {
                // A session reloads only with no transaction open: the dialog's own edits
                // will begin one.
                self.dialog_base = Some(self.revision);
                self.dialog_began = true;
                self.dialog = Some(dialog);
            }
            (_, _) => {
                self.dialog_base = None;
                self.message = Some(format!(
                    "The {} dialog closed: the file changed on disk",
                    dialog.kind.title()
                ));
            }
        }
    }

    /// The feature `id` (by its ID as written) as the timeline lists it, if it does.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.timeline
            .as_ref()?
            .features
            .as_ref()?
            .iter()
            .find(|entry| entry.id.as_str() == id)
    }

    /// Whether an error card's Edit would open the feature `id` now: for a sketch, Edit
    /// Sketch, if sketch mode may open ([`can_sketch`](Self::can_sketch)); for anything
    /// else, Edit Feature, if it has a dialog and one may open
    /// ([`can_open`](Self::can_open)).
    pub fn can_edit(&self, id: &FeatureId) -> bool {
        if self.is_sketch(id) {
            self.can_sketch()
        } else {
            self.kind_of(id).is_some_and(|kind| self.can_open(kind))
        }
    }

    /// An error card's Show in timeline: the feature `id` selected alone, and the timeline
    /// scrolled to its item once it's next drawn.
    pub fn show_in_timeline(&mut self, id: FeatureId) {
        self.selection.select_feature(id.clone());
        self.timeline_scroll = Some(id);
    }

    /// The feature the timeline is to scroll to, if one is.
    pub fn timeline_scroll(&self) -> Option<&FeatureId> {
        self.timeline_scroll.as_ref()
    }

    /// The timeline has scrolled to it (or had nothing to scroll), once.
    pub fn timeline_scrolled(&mut self) {
        self.timeline_scroll = None;
    }

    /// What an error card asked (`errCard`'s Edit and Show in timeline), done through
    /// `sessions`: whether a command was started.
    pub(crate) fn card_asked<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        asked: crate::properties::CardAct,
    ) -> bool {
        match asked {
            crate::properties::CardAct::Edit(id) => {
                // Edit Sketch for a sketch, as the timeline's double-click.
                self.edit_sketch_or_feature(sessions, &id);
                true
            }
            crate::properties::CardAct::Show(id) => {
                self.show_in_timeline(id);
                false
            }
            crate::properties::CardAct::Use { broken, candidate } => {
                self.use_candidate(sessions, &broken, &candidate)
            }
            crate::properties::CardAct::Remove(broken) => self.remove_reference(sessions, &broken),
        }
    }

    /// The fillet or chamfer `broken` is of, as the published model's document holds it,
    /// if that's the document the session is at, the card was read at it too, and nothing of
    /// the tab's is under way: what a broken reference's card mends. None otherwise.
    fn dressing(&self, broken: &crate::repair::Broken) -> Option<crate::repair::Dressing> {
        if !self.quiet() || broken.read != self.revision {
            return None;
        }
        let (model, revision) = self.selection.model()?;
        if revision != self.revision {
            return None;
        }
        crate::repair::Dressing::of(model.document(), &broken.feature)
    }

    /// The name the timeline gives the feature `id`, or its ID.
    fn named(&self, id: &FeatureId) -> String {
        self.entry(id.as_str())
            .map_or_else(|| id.to_string(), |entry| entry.name.clone())
    }

    /// Whether the open dialog edits the feature `id`.
    fn dialog_of(&self, id: &FeatureId) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.feature.as_ref() == Some(id))
    }

    /// What a card of `said` offers to mend a broken reference (flow 6b), if it's one with
    /// candidates (`errCard` offers Use and Remove the edge only then), read at the revision
    /// of the `check` it came from (every card's text is that check's): Remove the edge off
    /// for the feature's last edge, saying why, unless its dialog is open (whose box of
    /// names may be emptied, its OK off until an edge is picked, as SolidWorks' Fillet
    /// PropertyManager has it); and the candidate whose edge the canvas's pointer is over.
    pub(crate) fn repair(&self, said: &Said) -> Option<crate::properties::Repair> {
        let (read, _) = self.listed_for?;
        let broken = crate::repair::Broken::of(said, read)?;
        if broken.candidates.is_empty() {
            return None;
        }
        let remove = if self.dialog_of(&broken.feature) {
            Ok(())
        } else {
            self.dressing(&broken).map_or(Ok(()), |dressing| {
                dressing.removable(&self.named(&broken.feature))
            })
        };
        let marked = self.selection.hovered().and_then(|hovered| {
            broken
                .candidates
                .iter()
                .find(|name| {
                    self.selection.lit(name, &self.view) == Some((hovered.body, hovered.element))
                })
                .cloned()
        });
        Some(crate::properties::Repair {
            broken,
            remove,
            marked,
        })
    }

    /// The candidate whose card row the pointer is over this frame, said by a card drawn.
    pub(crate) fn hover_candidate(&mut self, name: Option<String>) {
        if self.candidate_next.is_none() {
            self.candidate_next = name;
        }
    }

    /// A new frame: the candidate the cards said last frame is the one the canvas lights.
    /// Whether that changed, so a frame is asked for to light it.
    pub(crate) fn take_candidate_hover(&mut self) -> bool {
        let next = self.candidate_next.take();
        if next == self.candidate_hot {
            return false;
        }
        self.candidate_hot = next;
        true
    }

    /// The candidate whose card row the pointer is over, as the canvas lights it.
    pub fn candidate_hovered(&self) -> Option<&str> {
        self.candidate_hot.as_deref()
    }

    /// A broken reference's `candidate` used in its place (`useCand`): read in the
    /// feature's base as its dialog reads a pick (the core's `resolve_in`), and written at
    /// the broken name's place in `edges` (`set_field` of `edges.N`, one undoable edit), the
    /// status bar saying so; or, its dialog open, put in the broken name's place in its box
    /// of names, which the dialog sends as its own edit. Refused, saying why, for a
    /// candidate that can't be stored there (unstable, or not on the base), a card read at
    /// another revision than the session's or while anything of the tab's is under way, a
    /// name the current model finds now, or a sketch open (its Cancel undoes to where it
    /// opened). Whether a command was started.
    pub(crate) fn use_candidate<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        broken: &crate::repair::Broken,
        candidate: &str,
    ) -> bool {
        if self.closing() {
            return false;
        }
        let name = self.named(&broken.feature);
        if self.dialog_of(&broken.feature) {
            return self.repair_in_dialog(sessions, broken, Some(candidate), &name);
        }
        if self.in_dialog() {
            self.hush();
            self.message = Some("Close the dialog before mending another feature".to_owned());
            return false;
        }
        if self.in_sketch() {
            // Its Cancel undoes to where it opened, a mend too, as a dialog's would.
            self.hush();
            self.message = Some("Finish or cancel the sketch first".to_owned());
            return false;
        }
        let Some(dressing) = self.dressing(broken) else {
            self.hush();
            self.message = Some(crate::repair::STALE.to_owned());
            return false;
        };
        let read = |name: &str, base: &FeatureId| self.selection.read_on(name, base);
        match crate::repair::used(broken, candidate, &dressing, &read, &name) {
            Ok((edit, told)) => {
                self.edit(sessions, edit);
                self.told = Some(told);
                true
            }
            Err(why) => {
                self.hush();
                self.message = Some(why);
                false
            }
        }
    }

    /// A broken reference's name dropped from its feature's `edges` (`removeRef`): one
    /// undoable edit setting `edges` without it, the status bar saying so; or, its dialog
    /// open, let go from its box of names. Never a closed feature's last edge, which is said;
    /// refused as Use is for a card of another revision, a name found now, or a sketch open.
    /// Whether a command was started.
    pub(crate) fn remove_reference<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        broken: &crate::repair::Broken,
    ) -> bool {
        if self.closing() {
            return false;
        }
        let name = self.named(&broken.feature);
        if self.dialog_of(&broken.feature) {
            return self.repair_in_dialog(sessions, broken, None, &name);
        }
        if self.in_dialog() {
            self.hush();
            self.message = Some("Close the dialog before mending another feature".to_owned());
            return false;
        }
        if self.in_sketch() {
            // Its Cancel undoes to where it opened, a mend too, as a dialog's would.
            self.hush();
            self.message = Some("Finish or cancel the sketch first".to_owned());
            return false;
        }
        let Some(dressing) = self.dressing(broken) else {
            self.hush();
            self.message = Some(crate::repair::STALE.to_owned());
            return false;
        };
        let read = |name: &str, base: &FeatureId| self.selection.read_on(name, base);
        match crate::repair::removed(broken, &dressing, &read, &name) {
            Ok((edit, told)) => {
                self.edit(sessions, edit);
                self.told = Some(told);
                true
            }
            Err(why) => {
                self.hush();
                self.message = Some(why);
                false
            }
        }
    }

    /// `broken` mended in the open dialog's box of names: `candidate`, read in the box's
    /// base, put in its place, or, for none, the name let go; then sent as the dialog sends
    /// its edits, the status bar saying what changed, and that the dialog's Cancel takes it
    /// back, only if something was sent (a box emptied sends nothing, its OK off until an
    /// edge is picked). Whether a command was started.
    fn repair_in_dialog<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        broken: &crate::repair::Broken,
        candidate: Option<&str>,
        name: &str,
    ) -> bool {
        let Some(dialog) = self.dialog.as_ref() else {
            return false;
        };
        let base = dialog
            .names_input()
            .and_then(|names| names.body.clone())
            .or_else(|| {
                self.entry(broken.feature.as_str())
                    .and_then(|entry| entry.inputs.first().cloned())
            });
        let stored = match (candidate, base) {
            (None, _) => None,
            (Some(candidate), Some(base)) => match self.selection.read_on(candidate, &base) {
                crate::selection::OnBase::Found(Ok(stored)) => Some(stored.to_string()),
                crate::selection::OnBase::Found(Err(refusal)) => {
                    self.hush();
                    self.message = Some(said(&Problems::from(refusal)));
                    return false;
                }
                crate::selection::OnBase::Missing => {
                    self.hush();
                    self.message = Some(format!(
                        "{candidate} isn't on {base}, which {name} is built on: it can't take \
                         that edge"
                    ));
                    return false;
                }
            },
            (Some(_), None) => {
                self.hush();
                self.message = Some(crate::repair::STALE.to_owned());
                return false;
            }
        };
        let chamfer = self
            .dialog
            .as_ref()
            .is_some_and(|d| d.kind == Kind::Chamfer);
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        let used = stored.is_some();
        if !dialog.repair(broken, stored) {
            self.hush();
            self.message = Some(crate::repair::STALE.to_owned());
            return false;
        }
        self.hush();
        // Sent first: an edit hushes what was said before it.
        let started = self.dialog_changed(sessions);
        if started {
            let made = crate::repair::Made::Dialog;
            self.told = Some(if used {
                crate::repair::used_words(name, chamfer, made)
            } else {
                crate::repair::removed_words(name, made)
            });
        }
        started
    }

    /// The dialog kind of the feature `id`, if the timeline lists it and a dialog edits it.
    pub fn kind_of(&self, id: &FeatureId) -> Option<Kind> {
        self.timeline
            .as_ref()?
            .features
            .as_ref()?
            .iter()
            .find(|entry| &entry.id == id)
            .and_then(|entry| Kind::of(entry.kind))
    }

    /// What the dialog's inputs ask, sent: its feature's add once they're valid, and then
    /// an edit of each field that changed, coalesced as the property panel's are. Whether
    /// anything was.
    pub fn dialog_changed<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        // An edit of its feature still to answer: its boxes of one body wait for it.
        let waiting = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref())
            .is_some_and(|feature| self.edits.pending_for(feature));
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        // What its boxes let go of, let go from the selection too.
        for name in dialog.take_unpicked() {
            self.selection.let_go(&name);
        }
        if let Some(members) = dialog.add() {
            let (serial, kind) = (dialog.serial, dialog.kind);
            self.edits.queue.push_back(Queued::Add {
                serial,
                kind,
                members,
                body: Body::Unread,
            });
            self.advance(sessions);
            return true;
        }
        let Some(feature) = dialog.feature.clone() else {
            return false;
        };
        let changes = dialog.changes(waiting);
        let changed = !changes.is_empty();
        for (field, value) in changes {
            self.edit(
                sessions,
                Edit {
                    feature: feature.clone(),
                    field: field.to_owned(),
                    value,
                },
            );
        }
        changed
    }

    /// The dialog closed: by OK, its edits kept, one step for undo; or by Cancel, undone
    /// back to the revision it opened at (`undo` with `to`, so edits made before it stay;
    /// rolled back if its own edits began the transaction). Neither while the tab closes:
    /// what it sent would come after the close.
    ///
    /// The selection the dialog's box of names took is put back as it was before by Cancel
    /// (read again once the Cancel's model is published, never against a preview of the
    /// dialog's edits, which may still land after it), and left as the box holds it by
    /// OK. The dialog OK'd is kept for the window to remember its values
    /// ([`take_oked`](Self::take_oked)).
    pub fn close_dialog<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, ok: bool) {
        if self.closing() {
            return;
        }
        let closed = self.dialog.take();
        if closed.is_none() && self.loading.take().is_none() {
            return;
        }
        let before = self.before_dialog.take();
        if ok {
            self.oked = closed;
        } else if let Some(items) = before {
            self.selection.restore(items);
        }
        self.edits
            .queue
            .push_back(Queued::Closed { ok, sketch: false });
        self.advance(sessions);
    }

    /// The last dialog OK'd since this was last asked, if one was: the window remembers its
    /// values for the next of its kind.
    pub fn take_oked(&mut self) -> Option<Dialog> {
        self.oked.take()
    }

    /// Whether a dialog is open, or its feature is being read for one.
    pub fn in_dialog(&self) -> bool {
        self.dialog.is_some() || self.loading.is_some()
    }

    /// Whether sketch mode is open.
    pub fn in_sketch(&self) -> bool {
        self.sketch.is_some()
    }

    /// Whether a dialog or a sketch is open: its own OK or Finish, or its Cancel, ends its
    /// edits, so Save and Save As wait for that (section 9), and the dialogs.
    pub fn editing(&self) -> bool {
        self.in_dialog() || self.in_sketch()
    }

    /// Whether sketch mode can open: as a dialog can, none open (nor a sketch), and the last
    /// one's Finish or Cancel taken, so what it opens at is after them.
    pub fn can_sketch(&self) -> bool {
        self.dialog.is_none()
            && self.loading.is_none()
            && self.sketch.is_none()
            && !self.closing()
            && !self
                .edits
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Closed { .. }))
            && self
                .timeline
                .as_ref()
                .is_some_and(|timeline| timeline.problems == 0)
    }

    /// Create Sketch on an origin plane, `plane` as the document writes it (`xy`, `xz`, or
    /// `yz`): sketch mode opened, and an empty sketch on it added (`add_feature`), as a
    /// dialog's feature is (section 9; fusion.md, *Sketch mode*).
    pub fn create_sketch<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        plane: silico_core::document::OriginPlane,
    ) {
        self.create_sketch_with(sessions, quoted(plane.as_str()));
    }

    /// Create Sketch on a planar face, by its element name `face` (M5.7's pick): its plane
    /// `{"face": face, "reversed": false}`, with no `x`, which `add_feature` fills in from
    /// the face (#348). Whether the name is a face, and planar, is the core's to say: a
    /// refusal is said, and sketch mode leaves.
    pub fn create_sketch_on_face<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        face: &str,
    ) {
        let plane = format!(r#"{{"face": {}, "reversed": false}}"#, quoted(face));
        self.create_sketch_with(sessions, plane);
    }

    /// Create Sketch on `plane`, as JSON writes it.
    fn create_sketch_with<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        plane: String,
    ) {
        if !self.can_sketch() {
            return;
        }
        // SolidWorks' and Fusion's names: Sketch1, Sketch2, and on.
        let name = format!("Sketch{}", self.browser.sketches.len() + 1);
        let text = format!(
            r#"{{"type": "sketch", "name": {}, "plane": {plane}, "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
            quoted(&name)
        );
        let Ok(feature) = silico_core::document::json::read(text.as_bytes()) else {
            // Never: the plane is JSON this module wrote.
            self.message = Some("The sketch couldn't be written".to_owned());
            return;
        };
        let serial = self.opening();
        self.sketch = Some(SketchMode::new(serial, None));
        self.edits
            .queue
            .push_back(Queued::AddSketch { serial, feature });
        self.advance(sessions);
    }

    /// Edit Sketch (section 9; docs/ux/solidworks.md, §5, *Editing upstream*): sketch mode
    /// opened on the sketch `id`, at the revision everything asked before it comes to.
    pub fn edit_sketch<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        id: &FeatureId,
    ) {
        if !self.can_sketch() || !self.is_sketch(id) {
            return;
        }
        let serial = self.opening();
        self.sketch = Some(SketchMode::new(serial, Some(id.clone())));
        self.selection.select_feature(id.clone());
        self.advance(sessions);
    }

    /// The timeline's Edit: Edit Sketch for a sketch, Edit Feature for anything else.
    pub fn edit_sketch_or_feature<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        id: &FeatureId,
    ) {
        if self.is_sketch(id) {
            self.edit_sketch(sessions, id);
        } else {
            self.edit_feature(sessions, id);
        }
    }

    /// Finish Sketch (`ok`), its edits kept, one step for undo; or Cancel, undone back to
    /// the revision sketch mode opened at, or rolled back if its own edits began the
    /// transaction: as a dialog's OK and Cancel are ([`close_dialog`](Self::close_dialog)).
    /// Neither while the tab closes.
    pub fn finish_sketch<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, ok: bool) {
        if self.closing() || self.sketch.is_none() {
            return;
        }
        // Its close waits for a drag's end (the queue waits while one is under way).
        self.leave_drag();
        self.sketch = None;
        self.drive_drag(sessions);
        self.edits
            .queue
            .push_back(Queued::Closed { ok, sketch: true });
        self.advance(sessions);
    }

    /// A drawing tool chosen in sketch mode (M5.11b's), or none: another than the one on
    /// starts afresh, the gesture under way ended with nothing added. Nothing outside
    /// sketch mode.
    pub fn sketch_tool(&mut self, tool: Option<silico_core::sketch_tools::ToolKind>) {
        self.sketch_choose(tool.map(Choice::Draw));
    }

    /// A tool chosen in sketch mode, a drawing tool, trim, or split, or none, as
    /// [`sketch_tool`](Self::sketch_tool) chooses one.
    pub fn sketch_choose(&mut self, choice: Option<Choice>) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.tools.choose(choice);
            // The canvas menu was of the tool before.
            mode.menu_at = None;
        }
    }

    /// A tool's button or its key in sketch mode: the tool chosen, or left if it's the one
    /// on, as SolidWorks' tool buttons and the prototype's `setTool` do. Nothing outside
    /// sketch mode.
    pub fn sketch_toggle(&mut self, choice: Choice) {
        if let Some(mode) = self.sketch.as_ref() {
            let on = mode.tools.choice() == Some(choice);
            self.sketch_choose((!on).then_some(choice));
        }
    }

    /// Construction's button or X in sketch mode: the palette's Construction turned on or
    /// off. Nothing outside sketch mode.
    pub fn sketch_toggle_construction(&mut self) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.settings.construction = !mode.settings.construction;
        }
    }

    /// One frame of the viewport's in sketch mode ([`SketchMode::frame`]): its left
    /// button's events given to the tool on, or to the selection, and the sketch drawn with
    /// `painter`; then what the tool asked sent, through the queue.
    pub fn sketch_frame<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        painter: &eframe::egui::Painter,
        input: &FrameInput<'_>,
    ) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.frame(painter, input, &mut self.drag);
        }
        self.sketch_requests(sessions);
        self.drive_drag(sessions);
    }

    /// The over-define prompt's `choice` (SD7): what it sends queued, against the revision
    /// its dimension's add or edit made, and the prompt closed. Nothing while the tab
    /// closes, whose queue takes no edit (the prompt closes all the same).
    pub fn sketch_offer<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        choice: OfferChoice,
    ) {
        if let Some(mode) = self.sketch.as_mut()
            && mode.answer_offer(choice)
        {
            self.message = None;
        }
        self.sketch_requests(sessions);
    }

    /// The Modify box asked `action` ([`SketchMode::modify`]); what it asks sent.
    pub fn sketch_modify<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        action: ModifyAction,
    ) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.modify(action);
        }
        self.sketch_requests(sessions);
    }

    /// Sketch mode's drag driven, never waited on: its begin sent once its press has moved
    /// and nothing of the tab's is under way (a transaction begun first, so its end's edit
    /// is held and undone as any other's), and its answers polled and its moves and end sent
    /// ([`Drag::poll`]); its refusals said, its end's edit taken as an edit's is.
    fn drive_drag<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let ready = self.quiet() && !self.closing() && self.sketch.is_some();
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        // Only from a read of the state the session is at: a press on an older one (an edit
        // since, its read still to come) drags nothing.
        if drag.wants_begin() && ready && self.state_of(drag.model) != self.state_of(self.revision)
        {
            if let Some(mode) = self.sketch.as_mut() {
                mode.let_go_press();
            }
            return;
        }
        if drag.wants_begin() && ready {
            self.message = None;
            self.begin_if_none(sessions);
            self.supersede_preview();
            self.record(Sent::SketchEdit("drag_begin"));
            self.drag_sending = Some(self.sending());
            drag.begun(sessions.drag_begin(&self.id, drag.sketch.clone(), drag.grab.clone()));
        }
        for news in drag.poll(sessions, &self.id) {
            match news {
                News::EndSent => self.record(Sent::SketchEdit("drag_end")),
                News::Refused(problems) => self.message = Some(said(&problems)),
                News::Ended(response) => {
                    self.took_session(&response);
                    if let Some(sending) = self.drag_sending.take() {
                        self.applied(sending, &response);
                    }
                    match &response.result {
                        Ok(ended) if ended.changed => {
                            if let Some(mode) = self.sketch.as_mut() {
                                mode.undone = 0;
                            }
                        }
                        Ok(_) => {}
                        Err(problems) => self.message = Some(said(problems)),
                    }
                }
            }
        }
        if self.sketch.is_none() {
            drag.unshown();
        }
        let in_flight = drag.in_flight();
        // Kept till it's over; then let go, so the next press may begin another.
        if self.drag.is_none() && !drag.over() {
            self.drag = Some(drag);
        }
        // Its end answered: what waited behind it (a close among them) goes now.
        if !in_flight {
            self.advance(sessions);
        }
    }

    /// Enter in sketch mode: a tool's chain ended, the tool kept (section 9, *Line*).
    pub fn sketch_enter<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.end_chain(silico_core::sketch_tools::Event::Enter);
        }
        self.sketch_requests(sessions);
    }

    /// The canvas menu's *End chain*: as Enter.
    pub fn sketch_end_chain<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.end_chain(silico_core::sketch_tools::Event::EndChain);
        }
        self.sketch_requests(sessions);
    }

    /// What sketch mode's tool asked, queued in order and sent as what's before it answers,
    /// never waited on: each add with its token (one whose members can't be written refused
    /// to the tool at once, and said), each trim, split, and extend of its sketch. A new
    /// edit drops what sketch mode undid, as Delete's does. Nothing while the tab closes,
    /// whose queue takes no edit, or with no sketch known: the tool's adds refused, so it
    /// holds none.
    fn sketch_requests<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let closing = self.closing();
        let Some(mode) = self.sketch.as_mut() else {
            return;
        };
        let asks = mode.take_asks();
        let serial = mode.serial;
        let mut asked = false;
        for ask in asks {
            if closing {
                continue;
            }
            let next = match ask {
                Ask::Value {
                    sketch,
                    constraint,
                    value,
                } => {
                    mode.undone = 0;
                    Queued::DimensionValue {
                        serial,
                        sketch,
                        constraint,
                        value,
                    }
                }
                Ask::Offer(request) => {
                    // Driven is an edit: Redo has nothing of sketch mode's to give back.
                    if matches!(request, OfferRequest::SetField { .. }) {
                        mode.undone = 0;
                    }
                    Queued::Offer { serial, request }
                }
            };
            self.message = None;
            self.edits.queue.push_back(next);
            asked = true;
        }
        let requests = mode.tools.take_requests();
        if requests.is_empty() {
            if asked {
                self.advance(sessions);
            }
            return;
        }
        let Some(sketch) = mode.sketch.clone() else {
            // No sketch to send them to (never, as a tool works only on a read of one):
            // each add refused to the tool, so it holds none it can't have answered.
            for request in requests {
                if let Request::Add { token, .. } = request {
                    mode.tools.refused(token);
                }
            }
            return;
        };
        let serial = mode.serial;
        let mut queued = false;
        for request in requests {
            let next = match request {
                Request::Add { token, members } => match NewMembers::of(&members) {
                    Ok(members) if !closing => Queued::SketchAdd {
                        serial,
                        sketch: sketch.clone(),
                        token,
                        members,
                    },
                    Ok(_) => {
                        mode.tools.refused(token);
                        continue;
                    }
                    Err(error) => {
                        mode.tools.refused(token);
                        self.message = Some(said(&Problems::from(error)));
                        continue;
                    }
                },
                _ if closing => continue,
                Request::Trim { curve, pick } => Queued::SketchTrim {
                    sketch: sketch.clone(),
                    curve,
                    pick,
                },
                Request::Split { curve, at } => Queued::SketchSplit {
                    sketch: sketch.clone(),
                    curve,
                    at,
                },
                Request::Extend { curve, end, by } => Queued::SketchExtend {
                    sketch: sketch.clone(),
                    curve,
                    end,
                    by,
                },
            };
            if !queued {
                mode.undone = 0;
                self.message = None;
                queued = true;
            }
            self.edits.queue.push_back(next);
        }
        if queued {
            self.advance(sessions);
        }
    }

    /// Delete in sketch mode: the selection removed (`sketch_remove`), or a lone split point
    /// joined (`sketch_join`), once what's asked before has answered; the selection let
    /// go. A refusal is said. Nothing with nothing selected, or before the sketch is known.
    pub fn sketch_delete<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if self.closing() {
            return;
        }
        let Some(mode) = self.sketch.as_mut() else {
            return;
        };
        let (Some(sketch), Some(deletion)) = (mode.sketch.clone(), mode.deletion()) else {
            return;
        };
        mode.selection.clear();
        // A new edit drops what was undone: Redo has nothing of sketch mode's to give back.
        mode.undone = 0;
        self.hush();
        self.edits.queue.push_back(match deletion {
            Deletion::Remove(members) => Queued::SketchRemove { sketch, members },
            Deletion::Join(point) => Queued::SketchJoin { sketch, point },
        });
        self.advance(sessions);
    }

    /// The browser's row of the feature `id` chosen while the dialog's active box takes a
    /// body: taken into it, if it's a body's row (one the dialog's own feature isn't built
    /// into), or refused, said under it; and the selection the bodies the boxes hold. Whether
    /// the box took the row (taken or refused), so it isn't selected as itself: false if no
    /// box of one body is active.
    pub fn pick_body(&mut self, id: &FeatureId) -> bool {
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
        if !dialog.takes_bodies() {
            return false;
        }
        let is_body = self.browser.bodies.iter().any(|row| &row.id == id);
        let entries = self
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_deref())
            .unwrap_or_default();
        let standing = standing(entries, id, dialog.feature.as_ref());
        dialog.took_row(id, is_body, standing);
        self.selection.select_features(dialog.bodies());
        true
    }

    /// An edit of `field` to `value` answered, `ok` if the core took it: if it's of the open
    /// dialog's feature, the dialog told (`Dialog::answered`), so a box of one body shows
    /// what the document holds. Whether a refused one's box went back to it.
    fn dialog_answered(&mut self, field: &(FeatureId, String), value: &Node, ok: bool) -> bool {
        self.dialog
            .as_mut()
            .filter(|dialog| dialog.feature.as_ref() == Some(&field.0))
            .is_some_and(|dialog| dialog.answered(&field.1, value, ok))
    }

    /// The sketch `id` chosen in the browser: the dialog's selection box takes it, if the
    /// dialog has one.
    pub fn pick(&mut self, id: &FeatureId) {
        if self.is_sketch(id)
            && let Some(dialog) = self.dialog.as_mut()
        {
            dialog.pick(id);
        }
    }

    /// Whether a `check` read is out, unanswered: what a test waits on.
    pub fn checking(&self) -> bool {
        self.check_read.is_some()
    }

    /// Whether a new sweep's body is being read (the document's features at the session's
    /// revision, for its add): what a test waits on.
    pub fn reading_body(&self) -> bool {
        self.under
            .iter()
            .any(|under| matches!(under, Under::Base { .. }))
    }

    /// Whether the dialog's inputs let it be OK'd: valid, no edit of one refused, and its
    /// add not unanswered (what's typed meanwhile is sent after it). While the tab closes,
    /// [`close_dialog`](Self::close_dialog) takes neither OK nor Cancel.
    pub fn can_ok(&self) -> bool {
        self.dialog.as_ref().is_some_and(|dialog| {
            !dialog.adding
                && !dialog.holding()
                && dialog.valid()
                && !dialog
                    .inputs
                    .iter()
                    .any(|input| self.field_refused(input.part.field()))
        })
    }

    /// Whether the last edit of `field` of the dialog's feature was refused.
    pub fn field_refused(&self, field: &str) -> bool {
        self.dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref())
            .is_some_and(|feature| self.refused.contains(&(feature.clone(), field.to_owned())))
    }

    /// What the dialog says beneath its inputs: its feature's error (or warning), as the
    /// preview's check gives it (its message, cause, elements, and suggestion), then the
    /// tab's last refusal.
    pub fn dialog_said(&self) -> Vec<Said> {
        let feature = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref());
        feature
            .and_then(|feature| self.said.get(feature.as_str()))
            .cloned()
            .into_iter()
            .chain(self.message.clone().map(|message| Said {
                message,
                ..Said::default()
            }))
            .collect()
    }

    /// The problems' text read (section 3, *When the app reads*): a document the gate
    /// refused, whose problems the property panel shows, as it's read; a feature's marks
    /// only once the person opens one (selects a marked feature), from a `check` of the
    /// published model.
    fn read_check<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let Some(timeline) = &self.timeline else {
            return;
        };
        let asked = (timeline.revision, timeline.model);
        if self.check_read.is_some() || self.checked == Some(asked) {
            return;
        }
        // The feature selected; or, with none, the open dialog's (a box of names keeps the
        // selection's elements, not the feature).
        let wanted = self.selection.feature().or_else(|| {
            self.dialog
                .as_ref()
                .and_then(|dialog| dialog.feature.as_ref())
        });
        let Some((gate, model)) = check_wanted(timeline, wanted, self.whats_wrong) else {
            return;
        };
        // Never the current model of a revision whose rebuild was stopped: its check would
        // start the rebuild again, undoing the stop. Asked once the rebuild runs again or
        // the timeline moves.
        if matches!(model, CheckModel::Current { .. }) && self.rebuild_stopped() {
            return;
        }
        self.checked = Some(asked);
        self.check_first = false;
        self.check_superseded = false;
        self.check_read = Some((gate, sessions.check(&self.id, model)));
    }

    /// What `answer` says of each feature, and of the document: all of a document the gate
    /// refused (`gate`), or else those of no feature; and every problem and warning, in the
    /// document's order, for What's Wrong.
    fn took_check(&mut self, answer: &CheckAnswer, gate: bool) {
        let mut by_feature = BTreeMap::new();
        let mut document = Vec::new();
        let mut listed = Vec::new();
        for error in answer.problems.errors() {
            let said = Said::of(error);
            if gate || error.feature().is_none() {
                document.push(said.clone());
            }
            if let Some(feature) = error.feature() {
                by_feature
                    .entry(feature.to_owned())
                    .or_insert_with(|| said.clone());
            }
            listed.push(said);
        }
        for warning in answer.warnings.listed() {
            let said = Said::of_warning(warning);
            if let Some(feature) = warning.feature() {
                by_feature
                    .entry(feature.to_owned())
                    .or_insert_with(|| said.clone());
            }
            listed.push(said);
        }
        let entries = self
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.features.as_deref())
            .unwrap_or_default();
        in_document_order(&mut listed, entries, gate);
        self.said = by_feature;
        self.document_problems = document;
        self.listed = listed;
        self.unlisted = Unlisted {
            errors: answer
                .problems
                .total()
                .saturating_sub(answer.problems.errors().len() as u64),
            warnings: answer
                .warnings
                .total()
                .saturating_sub(answer.warnings.listed().len() as u64),
        };
        self.listed_for = self.checked;
    }

    /// What What's Wrong lists: every problem and warning of the model the timeline shows,
    /// in the document's order; none if the timeline marks nothing. None while they're being
    /// read (or no timeline is read yet).
    pub fn listed(&self) -> Option<&[Said]> {
        let timeline = self.timeline.as_ref()?;
        if !marked(timeline) {
            return Some(&[]);
        }
        (self.listed_for == Some((timeline.revision, timeline.model)))
            .then_some(self.listed.as_slice())
    }

    /// How many errors and warnings the `check` [`listed`](Self::listed) answers counted
    /// past those it lists (the core lists the first hundred of each, and counts them all).
    pub fn unlisted(&self) -> Unlisted {
        self.unlisted
    }
}

/// Whether the solid of `body` is built through the feature `own`'s, as `entries` (the
/// timeline, in order) chain them: `body` is `own`, or one of its inputs is built through
/// it, followed back input by input. A fillet whose dialog is open is built into each body
/// this holds for, so a pick on one is read against the fillet's base.
fn built_through(entries: &[Entry], body: &FeatureId, own: &FeatureId) -> bool {
    let inputs: BTreeMap<&FeatureId, &[FeatureId]> = entries
        .iter()
        .map(|entry| (&entry.id, entry.inputs.as_slice()))
        .collect();
    let mut seen: std::collections::BTreeSet<&FeatureId> = std::collections::BTreeSet::new();
    let mut stack = vec![body];
    while let Some(at) = stack.pop() {
        if at == own {
            return true;
        }
        if seen.insert(at) {
            stack.extend(inputs.get(at).copied().unwrap_or_default());
        }
    }
    false
}

/// Where `body` stands to the open dialog's feature `own`, as `entries` (the timeline, in
/// order) have them: its own body or one built on it ([`built_through`]); after it, not
/// built on it, which it may not use; or before it, or with no feature of the dialog's yet
/// (a new one's add goes at the end).
fn standing(entries: &[Entry], body: &FeatureId, own: Option<&FeatureId>) -> Standing {
    let Some(own) = own else {
        return Standing::Before;
    };
    if built_through(entries, body, own) {
        return Standing::Through;
    }
    let place = |id: &FeatureId| entries.iter().position(|entry| &entry.id == id);
    match (place(body), place(own)) {
        (Some(body), Some(own)) if body > own => Standing::After,
        _ => Standing::Before,
    }
}

/// The body a new sweep builds on, as FreeCAD's PartDesign takes its body's tip, from a
/// document's features as `timeline` lists them, in order: the last that stands for a solid
/// and that no feature consumes. A feature stands for a solid if it makes one (it isn't a
/// sketch) and isn't suppressed, or if it's suppressed and its base stands for one (its
/// chain passes it through, as `document::suppression`'s `standing` has it). A feature is
/// consumed if a feature other than a sketch names it as an input, built or not, suppressed
/// or not, as the gate counts consumers (`Operation::inputs`: a feature's base and tool). A
/// sketch consumes nothing, so a sketch on a face leaves the face's feature standing; the
/// sketch a sweep names is among a sweep's inputs, and stands for nothing.
fn tip_of(entries: &[Entry]) -> Option<FeatureId> {
    let consumed: std::collections::BTreeSet<&FeatureId> = entries
        .iter()
        .filter(|entry| entry.kind != "sketch")
        .flat_map(|entry| entry.inputs.iter())
        .collect();
    let mut stands: BTreeMap<&FeatureId, bool> = BTreeMap::new();
    for entry in entries {
        let standing = if entry.kind == "sketch" {
            false
        } else if !entry.suppressed {
            true
        } else {
            // Its base: its first input (a boolean's base comes before its tool, and a
            // sweep's before its sketch; a sweep with no base has its sketch first, which
            // stands for nothing).
            entry
                .inputs
                .first()
                .and_then(|base| stands.get(base).copied())
                .unwrap_or(false)
        };
        stands.insert(&entry.id, standing);
    }
    entries
        .iter()
        .rev()
        .find(|entry| {
            stands.get(&entry.id).copied().unwrap_or(false) && !consumed.contains(&entry.id)
        })
        .map(|entry| entry.id.clone())
}

/// What a refused timeline read leaves, the tab having found nothing published before
/// (`nothing_published`): whether nothing is published now (the refusal names `model`), and
/// whether to ask again at once, which only a refusal that changes what the tab would ask
/// does: nothing published found, or found no longer, or the read cancelled (its rebuild
/// stopped, which the next read takes into account). Any other refusal (a rebuild that
/// failed with nothing published) waits for the revision or the published model to move,
/// so a read isn't sent again each frame.
fn after_refusal(nothing_published: bool, problems: &Problems) -> (bool, bool) {
    let now = problems
        .errors()
        .iter()
        .any(|error| error.parameter() == Some("model"));
    let cancelled = problems
        .errors()
        .iter()
        .any(|error| error.code() == silico_core::ErrorCode::Cancelled);
    (now, now != nothing_published || cancelled)
}

/// `listed` sorted into the document's order, as `entries` (the timeline) list its
/// features: the document's own first (all of a document the gate refused, `gate`, and any
/// of no feature), then each feature's, in its order, then any of a feature it doesn't
/// list; each feature's in the order the core gave them (its errors before its warnings).
fn in_document_order(listed: &mut [Said], entries: &[Entry], gate: bool) {
    let place: BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (entry.id.as_str(), at))
        .collect();
    // A stable sort: the core's order kept among equals.
    listed.sort_by_key(|said| match said.feature() {
        Some(feature) if !gate => place.get(feature).map_or((2, 0), |at| (1, *at)),
        _ => (0, 0),
    });
}

/// Whether a `check` of `timeline` is wanted, with `selected` the feature selected and
/// `listing` whether What's Wrong is open: for a document the gate refused (`gate`, true),
/// of the current model, its problems all the document's; or, for a selected feature
/// `timeline` marks or for What's Wrong while it marks anything, of the published model (the
/// current, while none is published). None for neither.
fn check_wanted(
    timeline: &TimelineAnswer,
    selected: Option<&FeatureId>,
    listing: bool,
) -> Option<(bool, CheckModel)> {
    let gate = timeline.problems > 0;
    let opened =
        (listing && marked(timeline))
            || selected.is_some_and(|selected| {
                timeline.features.iter().flatten().any(|entry| {
                    &entry.id == selected && (entry.problems > 0 || entry.warnings > 0)
                })
            });
    if !(gate || opened) {
        return None;
    }
    let model = if gate || timeline.model.is_none() {
        CheckModel::Current { fresh: false }
    } else {
        CheckModel::Published
    };
    Some((gate, model))
}

/// Whether `timeline` marks anything: the gate's problems, or a feature's problems or
/// warnings, whose text a `check` gives.
fn marked(timeline: &TimelineAnswer) -> bool {
    timeline.problems > 0
        || timeline.features.as_ref().is_some_and(|features| {
            features
                .iter()
                .any(|entry| entry.problems > 0 || entry.warnings > 0)
        })
}

/// The browser's rows of `timeline` (section 8): its bodies, the model's (`bodies`; a chain
/// whose last feature failed built no body there, and the core's timeline doesn't report
/// one, so none is listed for it), each marked by the worst of its chain's features (the
/// first in error, else the first with a warning); and its sketches, each marked by its own.
/// Each feature found by its ID once, through a map.
pub fn browser(timeline: &TimelineAnswer) -> Browser {
    let entries = timeline.features.as_deref().unwrap_or_default();
    let place: BTreeMap<&FeatureId, usize> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (&entry.id, at))
        .collect();
    let row = |entry: &Entry, worst: Option<usize>| Row {
        id: entry.id.clone(),
        name: entry.name.clone(),
        health: worst.map_or(Health::Healthy, |at| entries[at].health),
        marked_by: worst.map(|at| entries[at].id.clone()),
    };
    let bodies = timeline
        .bodies
        .iter()
        .filter_map(|id| place.get(id).copied())
        .map(|at| {
            let chain = chain(entries, &place, at);
            let worst = chain
                .iter()
                .copied()
                .find(|&i| entries[i].health == Health::Error)
                .or_else(|| {
                    chain
                        .iter()
                        .copied()
                        .find(|&i| entries[i].health == Health::Warning)
                });
            row(&entries[at], worst)
        })
        .collect();
    let sketches = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind == "sketch")
        .map(|(at, entry)| {
            let marked = matches!(entry.health, Health::Error | Health::Warning);
            row(entry, marked.then_some(at))
        })
        .collect();
    Browser { bodies, sketches }
}

/// The places of the features `entries[at]` is built from (its inputs, and theirs), with
/// its own, in timeline order.
fn chain(entries: &[Entry], place: &BTreeMap<&FeatureId, usize>, at: usize) -> Vec<usize> {
    let mut found = std::collections::BTreeSet::new();
    let mut to_visit = vec![at];
    while let Some(next) = to_visit.pop() {
        if !found.insert(next) {
            continue;
        }
        to_visit.extend(
            entries[next]
                .inputs
                .iter()
                .filter_map(|input| place.get(input)),
        );
    }
    found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::command::{Rebuild, RebuildState, Transaction};

    fn status(revision: Revision, changed: bool) -> Status {
        counted(revision, changed, 1, 0)
    }

    fn counted(revision: Revision, changed: bool, undo: usize, redo: usize) -> Status {
        Status {
            features: Some(1),
            gated: true,
            transaction: Some(Transaction {
                base: Revision::OPENED,
                changed,
                undo,
                redo,
                dropped: 0,
                bytes: 0,
            }),
            running: None,
            reads: Vec::new(),
            rebuild: Rebuild {
                revision,
                state: RebuildState::Idle,
                progress: None,
            },
            disk: None,
            drag: None,
        }
    }

    fn tab() -> Tab {
        let problems = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "none",
        ));
        let mut tab = Tab::opened(
            SessionId::parse("s1").unwrap(),
            "a.silico".to_owned(),
            &problems,
        );
        tab.transaction = true;
        tab
    }

    #[test]
    fn a_check_says_a_warnings_cause_and_elements_and_how_many_an_error_has_past_those_listed() {
        // An error listing 2 of its 150 elements, and a warning with its cause and 1 of its
        // 3: each kept whole for its feature's card, the warning marked as one, and each
        // card's Elements line saying how many there are past those listed (invariant 9).
        let error = silico_core::Error::new(silico_core::ErrorCode::KernelFailed, "Fillet failed")
            .with_feature("f0000001")
            .with_cause("OCCT couldn't fillet Edge1")
            .with_elements(["Edge1", "Edge2"])
            .with_elements_total(150)
            .with_suggestion("try a smaller radius");
        let warning = silico_core::Warning::new(
            silico_core::WarningCode::EndsNotJoined,
            "Two ends aren't joined",
        )
        .with_feature("s0000001")
        .with_cause("the ends are 1e-8 mm apart")
        .with_elements(["pa000001"])
        .with_elements_total(3)
        .with_suggestion("join them");
        let mut tab = tab();
        tab.took_check(
            &CheckAnswer {
                problems: Problems::from(error.clone()),
                warnings: silico_core::warnings::Warnings::of([warning.clone()]),
            },
            false,
        );
        let fillet = Said {
            warning: false,
            message: "Fillet failed".to_owned(),
            cause: Some("OCCT couldn't fillet Edge1".to_owned()),
            elements: vec!["Edge1".to_owned(), "Edge2".to_owned()],
            elements_total: 150,
            suggestion: Some("try a smaller radius".to_owned()),
            source: Some(Source::Error(error)),
        };
        assert_eq!(tab.said.get("f0000001"), Some(&fillet));
        assert_eq!(
            fillet.elements_line().as_deref(),
            Some("Elements: Edge1, Edge2 (the first 2 of 150)")
        );
        let sketch = Said {
            warning: true,
            message: "Two ends aren't joined".to_owned(),
            cause: Some("the ends are 1e-8 mm apart".to_owned()),
            elements: vec!["pa000001".to_owned()],
            elements_total: 3,
            suggestion: Some("join them".to_owned()),
            source: Some(Source::Warning(warning)),
        };
        assert_eq!(tab.said.get("s0000001"), Some(&sketch));
        assert_eq!(
            sketch.elements_line().as_deref(),
            Some("Elements: pa000001 (the first 1 of 3)")
        );
        // All listed: no count; none: no line.
        let all = Said {
            elements_total: 2,
            ..fillet.clone()
        };
        assert_eq!(
            all.elements_line().as_deref(),
            Some("Elements: Edge1, Edge2")
        );
        assert_eq!(Said::default().elements_line(), None);
    }

    #[test]
    fn a_body_is_built_through_a_feature_its_inputs_reach_back_to() {
        // A box, a fillet of it, a second box, and a fuse of the fillet with the second box;
        // and a body of its own. The fuse is built through the fillet and both boxes; the
        // fillet through itself; the lone body through none of them.
        let id = |id: &str| FeatureId::new(id).unwrap();
        let with = |at: &str, inputs: &[&str]| Entry {
            inputs: inputs.iter().map(|input| id(input)).collect(),
            ..entry(at, 0, 0)
        };
        let entries = [
            with("a0000001", &[]),
            with("f0000001", &["a0000001"]),
            with("b0000001", &[]),
            with("r0000001", &["f0000001", "b0000001"]),
            with("c0000001", &[]),
        ];
        let through = |body: &str, own: &str| built_through(&entries, &id(body), &id(own));
        assert!(through("r0000001", "f0000001"));
        assert!(through("r0000001", "a0000001"));
        assert!(through("r0000001", "b0000001"));
        assert!(through("f0000001", "f0000001"));
        assert!(!through("f0000001", "r0000001"));
        assert!(!through("c0000001", "f0000001"));
        assert!(!through("b0000001", "f0000001"));
        // A feature the timeline doesn't list yet (a new one's, before its read): itself.
        assert!(through("n0000001", "n0000001"));
        assert!(!through("n0000001", "f0000001"));
    }

    #[test]
    fn an_answer_newer_than_the_status_keeps_the_tab_unsaved_until_a_status_of_it_is_read() {
        // A status of revision 2 says nothing changed (an edit undone back, say); an answer
        // since brought the tab to revision 3, whose status isn't read yet: unsaved, as a
        // transaction is open, until a status of revision 3 says otherwise.
        let two = Revision::from_number(2).unwrap();
        let three = Revision::from_number(3).unwrap();
        let mut tab = tab();
        tab.status = Some(status(two, false));
        tab.status_revision = two;
        tab.revision = two;
        assert!(!tab.unsaved());
        tab.revision = three;
        assert!(tab.unsaved());
        tab.status = Some(status(three, false));
        tab.status_revision = three;
        assert!(!tab.unsaved());
    }

    #[test]
    fn undo_and_redo_are_off_while_an_answer_is_newer_than_the_status() {
        // A status of revision 2 counts one edit to undo and one to redo; an answer since
        // (a step's, say) brought the tab to revision 3: those counts aren't its own, so
        // neither is offered until a status of revision 3 is read.
        let two = Revision::from_number(2).unwrap();
        let three = Revision::from_number(3).unwrap();
        let mut tab = tab();
        tab.status = Some(counted(two, true, 1, 1));
        tab.status_revision = two;
        tab.revision = two;
        assert!(tab.undoable() && tab.redoable());
        tab.revision = three;
        assert!(!tab.undoable());
        assert!(!tab.redoable());
        tab.status = Some(counted(three, true, 1, 1));
        tab.status_revision = three;
        assert!(tab.undoable() && tab.redoable());
    }

    #[test]
    fn a_disk_finding_read_before_a_reload_seen_since_offers_no_choice() {
        // A status of revision 2 found the file changed with edits held: the choice offered.
        // An answer since said the session reloaded the file at 3 (Restore's `begin`, in the
        // frame that read that status): the finding compared the file with the bytes that
        // reload replaced, so nothing is offered until a status of 3 is read; one that finds
        // the file changed again offers it.
        let two = Revision::from_number(2).unwrap();
        let three = Revision::from_number(3).unwrap();
        let mut tab = tab();
        let mut found = status(two, true);
        found.disk = Some(Disk::Changed);
        tab.status = Some(found);
        tab.status_revision = two;
        tab.revision = two;
        tab.offer_disk_choice();
        assert_eq!(tab.disk_choice, DiskChoice::Offered(Disk::Changed));
        tab.reloaded_to = Some(three);
        tab.revision = three;
        tab.offer_disk_choice();
        assert_eq!(tab.disk_choice, DiskChoice::None);
        let mut again = status(three, true);
        again.disk = Some(Disk::Changed);
        tab.status = Some(again);
        tab.status_revision = three;
        tab.offer_disk_choice();
        assert_eq!(tab.disk_choice, DiskChoice::Offered(Disk::Changed));
    }

    fn entry(id: &str, problems: u64, warnings: u64) -> Entry {
        Entry {
            id: FeatureId::new(id).unwrap(),
            kind: "box",
            name: id.to_owned(),
            suppressed: false,
            inputs: Vec::new(),
            health: Health::Healthy,
            problems,
            warnings,
        }
    }

    fn timeline(problems: u64, model: Option<Revision>, features: Vec<Entry>) -> TimelineAnswer {
        TimelineAnswer {
            revision: Revision::OPENED,
            model,
            problems,
            features: Some(features),
            bodies: Vec::new(),
        }
    }

    #[test]
    fn a_timeline_is_marked_by_the_gates_problems_or_any_features_problems_or_warnings() {
        let clean = || vec![entry("a0000001", 0, 0), entry("b0000001", 0, 0)];
        assert!(!marked(&timeline(0, None, clean())));
        assert!(marked(&timeline(1, None, clean())));
        assert!(marked(&timeline(
            0,
            None,
            vec![entry("a0000001", 0, 0), entry("b0000001", 1, 0)]
        )));
        assert!(marked(&timeline(
            0,
            None,
            vec![entry("a0000001", 0, 1), entry("b0000001", 0, 0)]
        )));
        assert!(!marked(&TimelineAnswer {
            features: None,
            ..timeline(0, None, Vec::new())
        }));
    }

    #[test]
    fn a_check_is_wanted_for_a_refused_document_or_a_selected_marked_feature() {
        let one = Revision::OPENED;
        let a = FeatureId::new("a0000001").unwrap();
        let b = FeatureId::new("b0000001").unwrap();
        let features = || {
            vec![
                entry("a0000001", 0, 0),
                entry("b0000001", 0, 2),
                entry("c0000001", 3, 0),
            ]
        };
        // Nothing refused, nothing selected, or a healthy feature selected: none.
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), None, false),
            None
        );
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&a), false),
            None
        );
        // A selected feature with warnings, or problems: of the published model.
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&b), false),
            Some((false, CheckModel::Published))
        );
        let c = FeatureId::new("c0000001").unwrap();
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&c), false),
            Some((false, CheckModel::Published))
        );
        // None published: of the current one.
        assert_eq!(
            check_wanted(&timeline(0, None, features()), Some(&b), false),
            Some((false, CheckModel::Current { fresh: false }))
        );
        // A document the gate refused: the current model, whatever is selected.
        assert_eq!(
            check_wanted(&timeline(2, Some(one), features()), None, false),
            Some((true, CheckModel::Current { fresh: false }))
        );
        assert_eq!(
            check_wanted(&timeline(2, Some(one), features()), Some(&a), false),
            Some((true, CheckModel::Current { fresh: false }))
        );
    }

    #[test]
    fn whats_wrong_wants_a_check_whenever_the_timeline_marks_anything() {
        let one = Revision::OPENED;
        let marked = || vec![entry("a0000001", 0, 0), entry("b0000001", 0, 2)];
        let clean = || vec![entry("a0000001", 0, 0), entry("b0000001", 0, 0)];
        // A feature's warnings, nothing selected: of the published model while it's open,
        // and none while it's not.
        assert_eq!(
            check_wanted(&timeline(0, Some(one), marked()), None, true),
            Some((false, CheckModel::Published))
        );
        assert_eq!(
            check_wanted(&timeline(0, Some(one), marked()), None, false),
            None
        );
        // None published: of the current one.
        assert_eq!(
            check_wanted(&timeline(0, None, marked()), None, true),
            Some((false, CheckModel::Current { fresh: false }))
        );
        // Nothing marked: nothing to list, nothing read.
        assert_eq!(
            check_wanted(&timeline(0, Some(one), clean()), None, true),
            None
        );
    }

    fn failed(message: &str, feature: Option<&str>) -> silico_core::Error {
        let error = silico_core::Error::new(silico_core::ErrorCode::KernelFailed, message);
        match feature {
            Some(feature) => error.with_feature(feature),
            None => error,
        }
    }

    fn warned(message: &str, feature: &str) -> silico_core::Warning {
        silico_core::Warning::new(silico_core::WarningCode::FingerprintMissing, message)
            .with_feature(feature)
    }

    #[test]
    fn whats_wrong_lists_every_problem_and_warning_in_the_documents_order() {
        // The timeline: c, a, b, out of their IDs' order. The core gave a's error, b's
        // error, a problem of no feature, c's warning, a's warning, and a warning of a
        // feature it doesn't list.
        let entries = [
            entry("c0000001", 0, 1),
            entry("a0000001", 1, 1),
            entry("b0000001", 1, 0),
        ];
        let mut listed = vec![
            Said::of(&failed("a fails", Some("a0000001"))),
            Said::of(&failed("b fails", Some("b0000001"))),
            Said::of(&failed("the document fails", None)),
            Said::of_warning(&warned("c warns", "c0000001")),
            Said::of_warning(&warned("a warns", "a0000001")),
            Said::of_warning(&warned("z warns", "z0000001")),
        ];
        let said = |listed: &[Said]| -> Vec<(String, bool)> {
            listed
                .iter()
                .map(|said| (said.message.clone(), said.warning))
                .collect()
        };
        let mut gated = listed.clone();
        in_document_order(&mut listed, &entries, false);
        let want = [
            ("the document fails", false),
            ("c warns", true),
            ("a fails", false),
            ("a warns", true),
            ("b fails", false),
            ("z warns", true),
        ]
        .map(|(message, warning)| (message.to_owned(), warning));
        assert_eq!(said(&listed), want);
        // A document the gate refused: every problem the document's, in the core's order.
        in_document_order(&mut gated, &entries, true);
        assert_eq!(
            said(&gated)
                .iter()
                .map(|(message, _)| message.as_str())
                .collect::<Vec<_>>(),
            [
                "a fails",
                "b fails",
                "the document fails",
                "c warns",
                "a warns",
                "z warns"
            ]
        );
    }

    #[test]
    fn whats_wrong_lists_a_check_only_of_the_timeline_it_shows() {
        let one = Revision::OPENED;
        let mut tab = tab();
        tab.took_timeline(timeline(
            0,
            Some(one),
            vec![entry("a0000001", 0, 1), entry("b0000001", 1, 0)],
        ));
        // Not read yet: being read.
        assert_eq!(tab.listed(), None);
        tab.checked = Some((one, Some(one)));
        tab.took_check(
            &CheckAnswer {
                problems: Problems::from(failed("b fails", Some("b0000001"))),
                warnings: silico_core::warnings::Warnings::of([warned("a warns", "a0000001")]),
            },
            false,
        );
        let messages = |tab: &Tab| -> Option<Vec<String>> {
            tab.listed()
                .map(|listed| listed.iter().map(|said| said.message.clone()).collect())
        };
        assert_eq!(
            messages(&tab),
            Some(vec!["a warns".to_owned(), "b fails".to_owned()])
        );
        // Each kept whole, for its card's code and JSON.
        assert_eq!(
            tab.listed().unwrap()[1].source,
            Some(Source::Error(failed("b fails", Some("b0000001"))))
        );
        // Another revision's timeline: being read again.
        let mut moved = timeline(0, Some(one), vec![entry("b0000001", 1, 0)]);
        moved.revision = one.next().unwrap();
        tab.took_timeline(moved);
        assert_eq!(messages(&tab), None);
        // Nothing marked: nothing to list.
        tab.took_timeline(timeline(0, Some(one), vec![entry("a0000001", 0, 0)]));
        assert_eq!(messages(&tab), Some(Vec::new()));
        // A check of revision 1 that answers once the timeline shows revision 2: listed for
        // the revision it was asked of, so not for the one shown.
        tab.checked = Some((one, Some(one)));
        let mut later = timeline(0, Some(one), vec![entry("b0000001", 1, 0)]);
        later.revision = one.next().unwrap();
        tab.took_timeline(later);
        tab.took_check(
            &CheckAnswer {
                problems: Problems::from(failed("b fails", Some("b0000001"))),
                warnings: silico_core::warnings::Warnings::none(),
            },
            false,
        );
        assert_eq!(messages(&tab), None);
    }

    #[test]
    fn whats_wrong_counts_the_errors_and_warnings_the_core_counted_past_those_it_listed() {
        let one = Revision::OPENED;
        // 130 warnings: the core lists 100, and counts 30 more.
        let mut tab = tab();
        tab.took_timeline(timeline(0, Some(one), vec![entry("a0000001", 0, 130)]));
        tab.checked = Some((one, Some(one)));
        tab.took_check(
            &CheckAnswer {
                problems: Problems::from(failed("a fails", Some("a0000001"))),
                warnings: silico_core::warnings::Warnings::of(
                    (0..130).map(|_| warned("a warns", "a0000001")),
                ),
            },
            false,
        );
        assert_eq!(tab.listed().map(<[Said]>::len), Some(101));
        assert_eq!(
            tab.unlisted(),
            Unlisted {
                errors: 0,
                warnings: 30
            }
        );
        // A document the gate refuses for 130 boxes of no length: 100 errors listed, and 30
        // more counted.
        let boxes = (0..130)
            .map(|at| {
                format!(
                    r#"{{"id": "f{at:07}", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 0, "width": 1, "height": 1}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{boxes}]}}"#
        );
        let document = silico_core::document::read(text.as_bytes()).unwrap();
        let invalid = silico_core::document::validate(&document).unwrap_err();
        assert_eq!((invalid.errors().len(), invalid.total()), (100, 130));
        tab.took_timeline(timeline(130, Some(one), vec![entry("f0000000", 0, 0)]));
        tab.took_check(
            &CheckAnswer {
                problems: Problems::from(invalid),
                warnings: silico_core::warnings::Warnings::none(),
            },
            true,
        );
        assert_eq!(tab.listed().map(<[Said]>::len), Some(100));
        assert_eq!(
            tab.unlisted(),
            Unlisted {
                errors: 30,
                warnings: 0
            }
        );
        // Nothing marked: none counted.
        tab.took_timeline(timeline(0, Some(one), vec![entry("a0000001", 0, 0)]));
        assert_eq!(tab.unlisted(), Unlisted::default());
    }

    #[test]
    fn a_timeline_unmarked_drops_the_features_text_and_one_the_gate_took_the_documents() {
        let said = || Said {
            message: "m".to_owned(),
            ..Said::default()
        };
        let mut tab = tab();
        tab.said.insert("a0000001".to_owned(), said());
        tab.document_problems.push(said());
        // Marked by a feature's warning, the gate taking it: the feature's text kept, the
        // document's dropped.
        tab.took_timeline(timeline(0, None, vec![entry("a0000001", 0, 1)]));
        assert_eq!(tab.said.len(), 1);
        assert!(tab.document_problems.is_empty());
        // Refused by the gate: both kept.
        tab.document_problems.push(said());
        tab.took_timeline(timeline(1, None, vec![entry("a0000001", 0, 0)]));
        assert_eq!(tab.said.len(), 1);
        assert_eq!(tab.document_problems.len(), 1);
        // Nothing marked: the feature's text dropped too.
        tab.took_timeline(timeline(0, None, vec![entry("a0000001", 0, 0)]));
        assert!(tab.said.is_empty());
        assert!(tab.document_problems.is_empty());
    }

    #[test]
    fn a_feature_this_timeline_no_longer_marks_says_nothing_though_another_is_still_marked() {
        let said = |message: &str| Said {
            message: message.to_owned(),
            ..Said::default()
        };
        let mut tab = tab();
        tab.said.insert("a0000001".to_owned(), said("warned"));
        tab.said.insert("b0000001".to_owned(), said("in error"));
        tab.said.insert("c0000001".to_owned(), said("warned too"));
        // Marked still: the first by a warning, the third by an error; the second healed.
        tab.took_timeline(timeline(
            0,
            None,
            vec![
                entry("a0000001", 0, 1),
                entry("b0000001", 0, 0),
                entry("c0000001", 1, 0),
            ],
        ));
        let kept: Vec<(&str, &str)> = tab
            .said
            .iter()
            .map(|(id, said)| (id.as_str(), said.message.as_str()))
            .collect();
        assert_eq!(kept, [("a0000001", "warned"), ("c0000001", "warned too")]);
        // One the timeline no longer lists says nothing either.
        tab.took_timeline(timeline(0, None, vec![entry("a0000001", 0, 1)]));
        assert_eq!(tab.said.keys().collect::<Vec<_>>(), ["a0000001"]);
        // Under a document the gate refused, every feature's text is kept (the gate's
        // problems name features no timeline entry marks).
        tab.said.insert("b0000001".to_owned(), said("the gate's"));
        tab.took_timeline(timeline(1, None, vec![entry("a0000001", 0, 0)]));
        assert_eq!(tab.said.len(), 2);
    }

    /// A timeline entry of feature `id`, of `kind`, suppressed or not, naming `inputs`.
    fn feature(id: &str, kind: &'static str, suppressed: bool, inputs: &[&str]) -> Entry {
        Entry {
            id: FeatureId::new(id).unwrap(),
            kind,
            name: id.to_owned(),
            suppressed,
            inputs: inputs.iter().map(|i| FeatureId::new(i).unwrap()).collect(),
            health: Health::Healthy,
            problems: 0,
            warnings: 0,
        }
    }

    #[test]
    fn the_tip_is_what_stands_at_the_end_of_a_suppressed_chain_and_nothing_stands_for_none() {
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        // A box, a suppressed fillet of it, and a suppressed chamfer of the fillet: the
        // chamfer stands for the box, two links up, and nothing consumes it.
        let chain = [
            feature("b0000001", "box", false, &[]),
            feature("f0000001", "fillet", true, &["b0000001"]),
            feature("f0000002", "chamfer", true, &["f0000001"]),
        ];
        assert_eq!(tip_of(&chain), id("f0000002"));
        // A suppressed pad with no base, its sketch its first input: it stands for nothing,
        // and the box, which nothing consumes, is the tip.
        let pad = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &[]),
            feature("p0000001", "pad", true, &["s0000001"]),
        ];
        assert_eq!(tip_of(&pad), id("b0000001"));
        // A suppressed box, and a suppressed fuse of it with another box: the fuse stands
        // for nothing (its base doesn't), and the other box is consumed: no tip.
        let fused = [
            feature("b0000001", "box", true, &[]),
            feature("b0000002", "box", false, &[]),
            feature("z0000001", "fuse", true, &["b0000001", "b0000002"]),
        ];
        assert_eq!(tip_of(&fused), None);
        assert_eq!(tip_of(&[]), None);
    }

    #[test]
    fn a_sketch_on_a_face_consumes_nothing_and_leaves_its_faces_feature_the_tip() {
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        // A box, and a sketch on one of its faces: the timeline lists the face's feature among
        // the sketch's inputs, but the gate counts base and tool alone as consuming a solid,
        // and a sketch has neither: the box is the tip.
        let mut entries = vec![
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
        ];
        assert_eq!(tip_of(&entries), id("b0000001"));
        // A pocket of the box from that sketch: its base, the box, is consumed, and the pocket
        // is the tip.
        entries.push(feature(
            "p0000001",
            "pocket",
            false,
            &["b0000001", "s0000001"],
        ));
        assert_eq!(tip_of(&entries), id("p0000001"));
        // A sketch on a face of a box that a later fuse consumes: the fuse is the tip.
        let fused = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("b0000002", "box", false, &[]),
            feature("z0000001", "fuse", false, &["b0000001", "b0000002"]),
        ];
        assert_eq!(tip_of(&fused), id("z0000001"));
    }

    #[test]
    fn the_call_record_keeps_the_newest_it_holds() {
        let mut tab = tab();
        for at in 0..SENT_KEPT + 10 {
            tab.record(Sent::Edit(at.to_string()));
        }
        assert_eq!(tab.sent.len(), SENT_KEPT);
        assert_eq!(tab.sent.first(), Some(&Sent::Edit("10".to_owned())));
        assert_eq!(
            tab.sent.last(),
            Some(&Sent::Edit((SENT_KEPT + 9).to_string()))
        );
    }

    #[test]
    fn a_transactions_end_lets_go_of_its_dialogs_groups_and_the_states_its_steps_restored() {
        let mut tab = tab();
        let (one, two) = (Revision::OPENED, Revision::from_number(2).unwrap());
        tab.groups.push(Group {
            start: one,
            end: two,
        });
        tab.restored.insert(two, one);
        // Refused: kept.
        let mut close = false;
        tab.ended(
            Err(Problems::from(silico_core::Error::new(
                silico_core::ErrorCode::Internal,
                "no",
            ))),
            false,
            &mut close,
        );
        assert_eq!(tab.groups.len(), 1);
        assert_eq!(tab.restored.len(), 1);
        tab.ended(Ok(()), false, &mut close);
        assert!(tab.groups.is_empty());
        assert!(tab.restored.is_empty());
    }

    #[test]
    fn a_body_box_says_it_took_a_pick_and_nothing_once_nothing_more_is_toggled() {
        // A new union's box of one body takes the canvas's picks on the 2 by 1 by 1 box: a
        // face picked, the box takes its body, and says so (the canvas asks for a frame);
        // nothing toggled, before or since, it says nothing.
        use crate::selection::How;
        let mut tab = tab();
        let model = crate::selection::tests::box_model();
        tab.selection
            .published(Some((model, Revision::from_number(1).unwrap())), 1);
        let dialog = Dialog::new(Kind::Union, None, 1);
        tab.selection.take_into(dialog.takes());
        tab.dialog = Some(dialog);
        assert!(!tab.box_toggled(), "nothing toggled");
        tab.selection.pick("aaaaaaaa.box.Face1", How::Toggle);
        assert!(tab.box_toggled());
        assert_eq!(
            tab.dialog.as_ref().unwrap().bodies(),
            [FeatureId::new("aaaaaaaa").unwrap()]
        );
        assert!(!tab.box_toggled(), "nothing toggled since");
    }

    #[test]
    fn why_closed_says_two_bodies_only_for_a_boolean_with_fewer_and_the_documents_problems_first() {
        // Nothing before the timeline is read; then, for a boolean in a part of fewer than
        // two bodies, that it needs two, and for any other kind nothing; two are enough;
        // and a document the gate refused says that first, whatever the kind.
        let mut tab = tab();
        assert_eq!(tab.why_closed(Kind::Union), None, "no timeline read yet");
        let row = |id: &str| Row {
            id: FeatureId::new(id).unwrap(),
            name: id.to_owned(),
            health: Health::Healthy,
            marked_by: None,
        };
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::OPENED,
            model: Some(Revision::OPENED),
            problems: 0,
            features: Some(Vec::new()),
            bodies: Vec::new(),
        });
        assert_eq!(tab.why_closed(Kind::Union), Some(NEEDS_TWO_BODIES));
        assert_eq!(tab.why_closed(Kind::Box), None);
        tab.browser.bodies = vec![row("aaaaaaaa")];
        assert_eq!(tab.why_closed(Kind::Subtract), Some(NEEDS_TWO_BODIES));
        assert_eq!(tab.why_closed(Kind::Box), None);
        tab.browser.bodies.push(row("bbbbbbbb"));
        assert_eq!(tab.why_closed(Kind::Union), None, "two bodies are enough");
        tab.timeline.as_mut().unwrap().problems = 1;
        assert_eq!(tab.why_closed(Kind::Union), Some(UNMENDED));
        assert_eq!(tab.why_closed(Kind::Box), Some(UNMENDED));
    }

    #[test]
    fn a_let_go_the_box_takes_isnt_let_go_again_from_a_selection_that_took_it_back() {
        // A new fillet's box of edges takes the canvas's picks on the 2 by 1 by 1 box. Edge1
        // clicked: in the box and the selection. Then clicked twice before the box reads
        // the toggles: let go, then added again. The box lets it go and takes it again; the
        // let-go, removed from the box, isn't let go from the selection a second time, so
        // both hold Edge1, as after the first click.
        use crate::selection::How;
        let edge = "aaaaaaaa.box.Edge1";
        let mut tab = tab();
        let model = crate::selection::tests::box_model();
        tab.selection
            .published(Some((model, Revision::from_number(1).unwrap())), 1);
        let dialog = Dialog::new(Kind::Fillet, None, 1);
        tab.selection.take_into(dialog.takes());
        tab.dialog = Some(dialog);
        let held = |tab: &Tab| {
            let names = tab.dialog.as_ref().and_then(Dialog::names).unwrap();
            (tab.selection.names().join(" "), names.names.join(" "))
        };
        tab.selection.pick(edge, How::Toggle);
        assert!(tab.box_toggled());
        assert_eq!(held(&tab), (edge.to_owned(), edge.to_owned()));
        tab.selection.pick(edge, How::Toggle);
        tab.selection.pick(edge, How::Toggle);
        assert!(tab.box_toggled());
        assert_eq!(held(&tab), (edge.to_owned(), edge.to_owned()));
        assert!(!tab.box_toggled(), "nothing toggled since");
    }

    #[test]
    fn a_cancels_put_back_selection_is_read_from_its_undo_only_and_only_if_none_waits_behind_it() {
        // Edge1 picked on the box at revision 1, then put back by a Cancel: held, unread.
        // Each model published at revision 3 below is the box not named yet, so a read of
        // the selection against it leaves Edge1 waiting; unread, Edge1 stays found.
        use crate::selection::How;
        let edge = "aaaaaaaa.box.Edge1";
        let three = Revision::from_number(3).unwrap();
        // Found, by an answer of its own (a name waiting keeps the last it had, unstored).
        let found = |tab: &Tab| tab.selection.elements().next().unwrap().stable();
        let put_back = || {
            let mut tab = tab();
            let model = crate::selection::tests::box_model();
            tab.selection
                .published(Some((model, Revision::from_number(1).unwrap())), 1);
            tab.selection.pick(edge, How::Replace);
            let saved = tab.selection.saved();
            tab.selection.clear();
            tab.selection.restore(saved);
            tab
        };
        let unnamed = || {
            Some((
                Arc::new(crate::selection::tests::box_model().unnamed()),
                three,
            ))
        };
        // A Cancel, another queued behind it: not read from 3, the later Cancel's undo says
        // when.
        let mut tab = put_back();
        tab.edits.queue.push_back(Queued::Closed {
            ok: false,
            sketch: false,
        });
        tab.read_restored_from(false, three);
        tab.selection.published(unnamed(), 2);
        assert!(found(&tab), "read though another Cancel waits");
        // An OK: nothing put back by it, so nothing read from 3.
        let mut tab = put_back();
        tab.read_restored_from(true, three);
        tab.selection.published(unnamed(), 2);
        assert!(found(&tab), "read from an OK");
        // A Cancel alone: read from 3.
        let mut tab = put_back();
        tab.read_restored_from(false, three);
        tab.selection.published(unnamed(), 2);
        assert!(!found(&tab), "not read from the Cancel's undo");
    }

    #[test]
    fn a_refused_timeline_asks_again_only_when_the_refusal_changes_what_it_would_ask() {
        let refusal = |code: silico_core::ErrorCode, parameter: Option<&'static str>| {
            let error = silico_core::Error::new(code, "refused");
            Problems::from(match parameter {
                Some(parameter) => error.with_parameter(parameter),
                None => error,
            })
        };
        let model = refusal(silico_core::ErrorCode::InvalidInput, Some("model"));
        let failed = refusal(silico_core::ErrorCode::Internal, None);
        let cancelled = refusal(silico_core::ErrorCode::Cancelled, None);
        // Nothing published found: ask again (of the current model).
        assert_eq!(after_refusal(false, &model), (true, true));
        // Found again: nothing changes, nothing asked.
        assert_eq!(after_refusal(true, &model), (true, false));
        // Found no longer (a failure, say): ask again (of the published model).
        assert_eq!(after_refusal(true, &failed), (false, true));
        // A failure again: nothing asked until the revision or the model moves.
        assert_eq!(after_refusal(false, &failed), (false, false));
        // As a tab takes them.
        let mut tab = tab();
        let asked = Some((Revision::OPENED, None));
        tab.timeline_asked = asked;
        tab.took_refusal(&failed);
        assert_eq!(tab.timeline_asked, asked);
        tab.took_refusal(&model);
        assert_eq!(tab.timeline_asked, None);
        // A cancelled read: asked again, whatever it found.
        assert_eq!(after_refusal(false, &cancelled), (false, true));
        assert_eq!(after_refusal(true, &cancelled), (false, true));
    }

    #[test]
    fn the_export_dialog_opened_lets_go_of_the_last_ones_refusal_question_and_message() {
        use silico_core::command::export::ExportFormat;
        let mut tab = tab();
        tab.export_said = Some("refused".to_owned());
        tab.message = Some("refused".to_owned());
        tab.export_replacing = Some(Export {
            format: ExportFormat::Step,
            path: std::path::PathBuf::from("/work/part.step"),
            replace: false,
            features: None,
        });
        tab.export_opened();
        assert_eq!(tab.export_said, None);
        assert_eq!(tab.message, None);
        assert_eq!(tab.export_replacing, None);
    }

    #[test]
    fn an_export_answered_after_its_dialog_is_opened_again_closes_and_asks_nothing() {
        use silico_core::command::export::ExportFormat;
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        // STEP written by the fake, as the STEP worker would.
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(
            Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
                silico_core::cache::Cache::unsealed(dir)
            })
            .exporting_step(kernel),
        );
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        // A file at the path: the export is refused with replace's advice.
        let out = dir.path().join("part.step");
        std::fs::write(&out, "old").unwrap();
        let asked = Export {
            format: ExportFormat::Step,
            path: out.clone(),
            replace: false,
            features: None,
        };
        tab.export(&sessions, asked.clone());
        // Its answer is in once a command queued after it answers: then, before it's taken,
        // its dialog is cancelled (too late to stop it) and opened again.
        sessions.text(&id, None).wait().result.unwrap();
        tab.stop_export();
        tab.export_opened();
        tab.take_export();
        assert!(!tab.exporting());
        assert_eq!(tab.export_replacing, None);
        assert_eq!(tab.export_said, None);
        assert!(!tab.take_exported());
        // The next one, this dialog's own, asks.
        tab.export(&sessions, asked);
        sessions.text(&id, None).wait().result.unwrap();
        tab.take_export();
        assert!(tab.export_replacing.is_some());
        // One written (nothing at its path) and answered late, the same way: it doesn't
        // close the dialog opened since, nor say anything there.
        let fresh = dir.path().join("fresh.step");
        tab.stop_export();
        tab.export(
            &sessions,
            Export {
                format: ExportFormat::Step,
                path: fresh.clone(),
                replace: false,
                features: None,
            },
        );
        sessions.text(&id, None).wait().result.unwrap();
        tab.stop_export();
        tab.export_opened();
        tab.take_export();
        assert!(fresh.is_file());
        assert!(tab.export_answer.as_ref().is_some_and(Result::is_ok));
        assert!(!tab.take_exported());
        assert_eq!(tab.export_said, None);
    }

    #[test]
    fn the_exports_sent_keep_the_newest_it_holds() {
        use silico_core::command::export::ExportFormat;
        // A session that isn't open: each export answered at once, refused.
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        let mut tab = tab();
        for at in 0..SENT_KEPT + 10 {
            tab.export(
                &sessions,
                Export {
                    format: ExportFormat::Step,
                    path: std::path::PathBuf::from(format!("/nowhere/{at}.step")),
                    replace: false,
                    features: None,
                },
            );
            tab.take_export();
            assert!(!tab.exporting());
        }
        assert_eq!(tab.exports.len(), SENT_KEPT);
        assert_eq!(
            tab.exports[0].path,
            std::path::PathBuf::from("/nowhere/10.step")
        );
    }

    #[test]
    fn an_answer_moves_the_tabs_revision_and_transaction_forward_never_back() {
        let info = |revision: u64, transaction: bool| silico_core::command::SessionInfo {
            id: SessionId::parse("s1").unwrap(),
            path: Some("a.silico".to_owned()),
            revision: Revision::from_number(revision).unwrap(),
            transaction,
            reloaded: false,
        };
        let response = |revision: u64, transaction: bool| Response::<()> {
            session: Some(info(revision, transaction)),
            model: None,
            result: Ok(()),
        };
        let mut tab = tab();
        tab.transaction = false;
        tab.took_session(&response(3, true));
        assert_eq!(tab.revision.number(), 3);
        assert!(tab.transaction);
        // An older answer taken after: nothing moves back.
        tab.took_session(&response(2, false));
        assert_eq!(tab.revision.number(), 3);
        assert!(tab.transaction);
    }

    #[test]
    fn a_check_is_read_once_for_each_revision_and_model() {
        // A selected feature with a warning: one check read; once it has answered, none
        // again for the same timeline's revision and model.
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        let mut tab = tab();
        tab.timeline = Some(timeline(
            0,
            Some(Revision::OPENED),
            vec![entry("a0000001", 0, 1)],
        ));
        tab.selection_mut()
            .select_feature(FeatureId::new("a0000001").unwrap());
        tab.read_check(&sessions);
        assert!(tab.check_read.is_some());
        // Answered and taken.
        tab.check_read = None;
        tab.read_check(&sessions);
        assert!(tab.check_read.is_none());
        // A new revision's timeline: read again.
        tab.timeline = Some(TimelineAnswer {
            revision: Revision::from_number(2).unwrap(),
            ..timeline(0, Some(Revision::OPENED), vec![entry("a0000001", 0, 1)])
        });
        tab.read_check(&sessions);
        assert!(tab.check_read.is_some());
    }

    #[test]
    fn no_check_of_the_current_model_is_asked_while_its_rebuild_is_stopped() {
        // A timeline naming no model, a marked feature selected: a check of the current
        // model, which would start a stopped rebuild again. Not asked while `status` says
        // the tab's revision's rebuild stopped; asked in every other state, and a check of
        // the published model asked even then.
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        let at = |state, model| {
            let mut tab = tab();
            tab.revision = Revision::OPENED;
            let mut status = status(Revision::OPENED, true);
            status.rebuild.state = state;
            tab.status = Some(status);
            tab.timeline = Some(timeline(0, model, vec![entry("a0000001", 0, 1)]));
            tab.selection_mut()
                .select_feature(FeatureId::new("a0000001").unwrap());
            tab.read_check(&sessions);
            tab.check_read.is_some()
        };
        assert!(!at(RebuildState::Stopped, None));
        for state in [
            RebuildState::Idle,
            RebuildState::Running,
            RebuildState::Done,
            RebuildState::Faulted,
        ] {
            assert!(at(state, None), "{state:?}");
        }
        assert!(at(RebuildState::Stopped, Some(Revision::OPENED)));
    }

    #[test]
    fn a_check_the_stop_cancels_is_asked_again_of_the_same_timeline() {
        // A check out when the stop comes, cancelled through its handle so it never starts a
        // stopped rebuild again: once it has answered `cancelled`, the same timeline asks it
        // again, its marks' text still to be read.
        use silico_core::command::export::ExportFormat;
        use silico_core::kernel::Op;
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(
            Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
                silico_core::cache::Cache::unsealed(dir)
            })
            .exporting_step(Arc::clone(&kernel)),
        );
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        // Built: the export below waits on its STEP alone, no rebuild running.
        sessions
            .timeline(&id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        // The queue held behind an export whose STEP the fake holds: a check sent now waits
        // there, unanswered.
        let hold = kernel.hold(|op| matches!(op, Op::ExportStep(_)));
        tab.export(
            &sessions,
            Export {
                format: ExportFormat::Step,
                path: dir.path().join("part.step"),
                replace: false,
                features: None,
            },
        );
        assert!(kernel.wait_running(1, std::time::Duration::from_secs(60)));
        tab.timeline = Some(timeline(0, None, vec![entry("k7f2qx3m", 0, 1)]));
        tab.selection_mut()
            .select_feature(FeatureId::new("k7f2qx3m").unwrap());
        tab.read_check(&sessions);
        assert!(tab.check_read.is_some());
        // The stop: no rebuild running, so refused, but the check cancelled.
        assert!(tab.halt(&sessions).is_err());
        let (_, read) = tab.check_read.take().unwrap();
        hold.release();
        assert_eq!(
            read.wait().result.unwrap_err().errors()[0].code(),
            silico_core::ErrorCode::Cancelled
        );
        // Taken as `take_answers` takes it: the same timeline asks it again.
        tab.read_check(&sessions);
        assert!(tab.check_read.is_some());
    }

    #[test]
    fn a_check_of_an_older_timeline_taken_in_the_frame_of_a_newer_one_brings_back_nothing() {
        // Two boxes with no fingerprint, each warned. The first selected: its check out,
        // then the first removed, then the timeline of the revision that made, all answered
        // before the frame takes them, the timeline first. That timeline marks only the
        // second: the first's warning, the check's, isn't brought back.
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}, {"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [10, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        let first = sessions
            .timeline(&id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        let warned = |timeline: &TimelineAnswer| -> Vec<String> {
            timeline
                .features
                .iter()
                .flatten()
                .filter(|entry| entry.warnings > 0)
                .map(|entry| entry.id.as_str().to_owned())
                .collect()
        };
        assert_eq!(warned(&first), ["a0000001", "b0000001"]);
        tab.took_timeline(first);
        tab.selection_mut()
            .select_feature(FeatureId::new("a0000001").unwrap());
        tab.read_check(&sessions);
        assert!(tab.check_read.is_some());
        // Queued behind the check, in turn on the session's thread.
        let removed =
            sessions.remove_feature(&id, Revision::OPENED, FeatureId::new("a0000001").unwrap());
        tab.timeline_read = Some(sessions.timeline(&id, ModelChoice::Current));
        assert!(removed.wait().result.unwrap().changed);
        // Queued after both reads: once it answers, so have they.
        sessions.text(&id, None).wait().result.unwrap();
        tab.selection_mut().clear();
        tab.take_answers(&sessions);
        assert!(tab.timeline_read.is_none() && tab.check_read.is_none());
        let newest = tab.timeline.as_ref().unwrap();
        assert_ne!(newest.revision, Revision::OPENED);
        assert_eq!(warned(newest), ["b0000001"]);
        assert!(
            !tab.said.contains_key("a0000001"),
            "{:?}",
            tab.said.keys().collect::<Vec<_>>()
        );
        assert!(tab.document_problems.is_empty());
    }

    #[test]
    fn a_read_the_tab_superseded_is_never_counted_as_the_stops() {
        // The tab's read of the current model, its rebuild's waiter, superseded (as an edit
        // sent supersedes it), which stops the rebuild. The Stop asked after finds nothing
        // running: its refusal is said, the read's cancel being the tab's own, not the stop's.
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        let hold = kernel.hold(|_| true);
        tab.timeline_read = Some(sessions.timeline(&id, ModelChoice::Current));
        let state = |sessions: &Arc<Sessions<_>>| {
            sessions
                .status(&id)
                .result
                .map(|status| (status.running, status.rebuild.state))
                .unwrap()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let until = |done: &dyn Fn() -> bool| {
            while !done() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        until(&|| {
            state(&sessions)
                == (
                    Some(silico_core::command::RunningCommand::Command(
                        silico_core::command::SessionCommand::Timeline,
                    )),
                    RebuildState::Running,
                )
        });
        tab.supersede_preview();
        until(&|| state(&sessions).1 == RebuildState::Stopped);
        tab.stop(&sessions);
        let said = tab.message.clone().unwrap_or_default();
        assert!(said.contains("no rebuild is running in session"), "{said}");
        hold.release();
    }

    const HELD_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// On this thread until `done` holds, within [`HELD_WAIT`].
    #[track_caller]
    fn until(what: &str, done: &dyn Fn() -> bool) {
        let deadline = std::time::Instant::now() + HELD_WAIT;
        while !done() {
            assert!(std::time::Instant::now() < deadline, "never {what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A session of one box, unbuilt, its tab, and every kernel request held from here on.
    struct HeldBox {
        _dir: tempfile::TempDir,
        kernel: Arc<silico_core::fake::FakeKernel>,
        sessions: Arc<Sessions<Arc<silico_core::fake::FakeKernel>>>,
        id: SessionId,
        tab: Tab,
        hold: silico_core::fake::Hold,
    }

    impl HeldBox {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let document = dir.path().join("part.silico");
            std::fs::write(
                &document,
                r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
            )
            .unwrap();
            let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
            let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
                silico_core::cache::Cache::unsealed(dir)
            }));
            let opened = sessions.open(&document, false).wait();
            let id = opened.session.unwrap().id;
            let tab = Tab::opened(
                id,
                document.to_str().unwrap().to_owned(),
                &opened.result.unwrap().problems,
            );
            let hold = kernel.hold(|_| true);
            HeldBox {
                _dir: dir,
                kernel,
                sessions,
                id,
                tab,
                hold,
            }
        }

        /// How many times the box was asked of the kernel.
        fn boxes(&self) -> usize {
            boxes(&self.kernel)
        }

        /// The tab's timeline read sent, of the current model, nothing being published.
        fn send_timeline(&mut self) {
            self.tab.nothing_published = true;
            self.tab.read_timeline(&self.sessions);
            assert!(self.tab.timeline_read.is_some());
        }

        /// The tab's check read sent: of the current model with a timeline naming no model,
        /// of the published one with one naming the opened revision's; a marked feature
        /// selected either way.
        fn send_check(&mut self, model: Option<Revision>) {
            self.tab.timeline = Some(timeline(0, model, vec![entry("k7f2qx3m", 0, 1)]));
            self.tab
                .selection_mut()
                .select_feature(FeatureId::new("k7f2qx3m").unwrap());
            self.tab.read_check(&self.sessions);
            assert!(self.tab.check_read.is_some());
        }

        /// Until `command` is the session's queued command running, waiting on the rebuild,
        /// the box held.
        fn waiting(&self, command: silico_core::command::SessionCommand) {
            until("the read waiting on the rebuild", &|| {
                self.sessions.status(&self.id).result.is_ok_and(|status| {
                    status.running == Some(silico_core::command::RunningCommand::Command(command))
                        && status.rebuild.state == RebuildState::Running
                })
            });
            assert!(self.kernel.wait_running(1, HELD_WAIT));
            assert_eq!(self.boxes(), 1);
        }

        /// Both of the tab's reads taken, each answered `cancelled`; then nothing of the
        /// session's is queued, the box was asked once, and the rebuild is stopped.
        fn both_cancelled_and_stopped(&mut self, case: &str) {
            let timeline = self.tab.timeline_read.take().unwrap().wait();
            let (_, check) = self.tab.check_read.take().unwrap();
            let check = check.wait();
            assert_eq!(
                timeline.result.unwrap_err().errors()[0].code(),
                silico_core::ErrorCode::Cancelled,
                "{case}"
            );
            assert_eq!(
                check.result.unwrap_err().errors()[0].code(),
                silico_core::ErrorCode::Cancelled,
                "{case}"
            );
            assert_eq!(self.boxes(), 1, "{case}");
            let status = self.sessions.status(&self.id).result.unwrap();
            assert_eq!(status.running, None, "{case}");
            assert_eq!(status.rebuild.state, RebuildState::Stopped, "{case}");
        }
    }

    #[test]
    fn the_stop_cancels_the_tabs_read_sent_later_first_so_neither_starts_the_rebuild_again() {
        // The tab's timeline and check reads of the current model, one the rebuild's waiter
        // (the box held), the other queued behind it. The stop is held after its first
        // cancel until the session has run what that cancel lets it run. Had it cancelled the
        // waiter first, the waiter would stop the rebuild and answer, and the session go on
        // to the other read, not yet cancelled, which starts the stopped rebuild again
        // (decision 34): the box asked of the kernel a second time. Cancelled the one sent
        // later first, the box is asked once, both reads answer `cancelled`, and the rebuild
        // stays stopped: the timeline read sent first; the check sent first; and a check
        // sent first and answered (of the published model, at once), then the timeline
        // read, then a check of the current model sent again behind it, so the check out
        // is the later, whatever the first was.
        use silico_core::command::SessionCommand;
        for case in ["timeline first", "check first", "check sent again"] {
            let mut held = HeldBox::new();
            let waiter = match case {
                "timeline first" => {
                    held.send_timeline();
                    held.waiting(SessionCommand::Timeline);
                    held.send_check(None);
                    OwnRead::Timeline
                }
                "check first" => {
                    held.send_check(None);
                    held.waiting(SessionCommand::Check);
                    held.send_timeline();
                    OwnRead::Check
                }
                _ => {
                    // Of the published model: it starts the rebuild, which the read sent
                    // next waits on, and is answered at once, nothing being published.
                    held.send_check(Some(Revision::OPENED));
                    held.send_timeline();
                    assert!(held.tab.check_first);
                    held.waiting(SessionCommand::Timeline);
                    let (_, first) = held.tab.check_read.take().unwrap();
                    assert!(first.wait().result.is_err());
                    held.send_check(None);
                    OwnRead::Timeline
                }
            };
            assert_eq!(held.tab.check_first, waiter == OwnRead::Check, "{case}");
            let kernel = Arc::clone(&held.kernel);
            let stopped = held.tab.halt_with(&held.sessions, |cancelled| {
                // The waiter cancelled, the other read still uncancelled: the session runs it
                // next, which starts the rebuild again, asking for the box.
                if cancelled == waiter {
                    until("the stopped rebuild started again", &|| boxes(&kernel) > 1);
                }
            });
            assert!(stopped.is_ok(), "{case}");
            held.both_cancelled_and_stopped(case);
            held.hold.release();
        }
    }

    #[test]
    fn rebuild_records_whether_a_check_was_out_before_its_read() {
        // Rebuild's read of the current model sent with a check out: the check was sent
        // first, so the stop cancels Rebuild's read first; with none out, not.
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        for out in [true, false] {
            let mut tab = tab();
            tab.revision = Revision::OPENED;
            let mut stopped = status(Revision::OPENED, true);
            stopped.rebuild.state = RebuildState::Stopped;
            tab.status = Some(stopped);
            if out {
                tab.check_read = Some((false, sessions.check(&tab.id, CheckModel::Published)));
            }
            // Whatever it said before.
            tab.check_first = !out;
            assert!(tab.can_rebuild());
            tab.rebuild(&sessions);
            assert!(tab.timeline_read.is_some());
            assert_eq!(tab.check_first, out);
        }
    }

    #[test]
    fn an_edit_cancels_the_tabs_check_queued_behind_its_preview_read_so_the_old_rebuild_stays_stopped()
     {
        // The preview's timeline read of the current model, the rebuild's waiter (the box
        // held), and a check of the current model queued behind it. An edit supersedes both,
        // the check first: left, it would run once the read stopped the rebuild and start it
        // again, so the edit would wait for a rebuild of a revision it makes old. Both answer
        // `cancelled`, the box is asked once, and a stop after counts neither cancel as its
        // own (each is the edit's): it finds nothing to stop, and says so.
        use silico_core::command::SessionCommand;
        let mut held = HeldBox::new();
        held.send_timeline();
        held.waiting(SessionCommand::Timeline);
        held.send_check(None);
        held.tab.supersede_preview();
        until("the session done with both reads", &|| {
            held.boxes() > 1
                || held.sessions.status(&held.id).result.is_ok_and(|status| {
                    status.running.is_none() && status.rebuild.state == RebuildState::Stopped
                })
        });
        assert_eq!(held.boxes(), 1, "the old revision's rebuild started again");
        let said = held.tab.halt(&held.sessions).unwrap_err();
        assert!(
            said.errors()[0]
                .message()
                .contains("no rebuild is running in session"),
            "{}",
            said.errors()[0].message()
        );
        held.both_cancelled_and_stopped("superseded");
        held.hold.release();
    }

    /// How many times a box was asked of `kernel`.
    fn boxes(kernel: &silico_core::fake::FakeKernel) -> usize {
        use silico_core::kernel::{Op, Primitive};
        kernel
            .asked()
            .iter()
            .filter(|call| matches!(call.op, Op::MakePrimitive(Primitive::Box { .. })))
            .count()
    }

    // ---- #382's mutants: Rebuild everything's state, its problems, and nothing published ----

    /// A document of one block, as the app's tests write one.
    const BLOCK: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}, {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [2, 1, -1], "radius": 0.5, "height": 3}, {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}]}"#;

    type FakeSessions = Arc<Sessions<Arc<silico_core::fake::FakeKernel>>>;

    /// Sessions on a fake kernel, `kernel` first told what to do, and `text` opened in a
    /// tab: its directory (kept for the session's file), the sessions, and the tab.
    fn opened_on(
        text: &str,
        kernel: impl FnOnce(&silico_core::fake::FakeKernel),
    ) -> (tempfile::TempDir, FakeSessions, Tab) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(&document, text).unwrap();
        let fake = silico_core::fake::FakeKernel::new(1).unwrap();
        kernel(&fake);
        let kernel = Arc::new(fake);
        let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        (dir, sessions, tab)
    }

    /// Answers taken until `done` holds, within a minute.
    fn taken_until(tab: &mut Tab, sessions: &FakeSessions, done: impl Fn(&Tab) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(tab) {
            tab.take_answers(sessions);
            assert!(std::time::Instant::now() < deadline, "never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// How many Restores, and the put-backs of their text, `tab` holds, queued or sent.
    fn restores(tab: &Tab) -> usize {
        let queued = tab
            .edits
            .queue
            .iter()
            .filter(|queued| matches!(queued, Queued::Restore { .. } | Queued::Replace { .. }))
            .count();
        let under = tab
            .under
            .iter()
            .filter(|under| matches!(under, Under::Restore { .. } | Under::Replace { .. }))
            .count();
        queued + under
    }

    #[test]
    fn restore_is_asked_once_never_in_a_dialog_or_a_close_and_its_put_back_resets_the_fields() {
        // Fable's decision of 2026-11-04, section 2: Restore is never asked while a dialog is
        // open or the tab closes, and once while one is under way; its text put back, an
        // edit of the document other than the property panel's own, resets the panel's
        // fields, as an undo, a redo, a revert, or a reload does.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        let one = Revision::OPENED;
        // A dialog open, its feature's text still read: nothing asked.
        tab.loading = Some(1);
        tab.restore(&sessions, one);
        assert_eq!(restores(&tab), 0, "asked in a dialog");
        tab.loading = None;
        // Sketch mode open: nothing asked.
        tab.sketch = Some(SketchMode::new(1, None));
        tab.restore(&sessions, one);
        assert_eq!(restores(&tab), 0, "asked in sketch mode");
        tab.sketch = None;
        // The tab closing: nothing asked.
        tab.edits.queue.push_back(Queued::Save { then_close: true });
        tab.restore(&sessions, one);
        assert_eq!(restores(&tab), 0, "asked as the tab closes");
        tab.edits.queue.clear();
        // Asked: under way until its text is put back, and asked again meanwhile, nothing
        // more.
        let resets = tab.resets;
        tab.restore(&sessions, one);
        assert!(tab.restoring(), "the read asked");
        assert_eq!(restores(&tab), 1);
        tab.restore(&sessions, one);
        assert_eq!(restores(&tab), 1, "asked twice");
        // Put back in a transaction begun for it.
        taken_until(&mut tab, &sessions, |tab| {
            tab.under
                .iter()
                .any(|under| matches!(under, Under::Replace { .. }))
        });
        assert!(tab.restoring(), "the text put back");
        tab.restore(&sessions, one);
        assert_eq!(restores(&tab), 1, "asked while its text is put back");
        assert!(matches!(
            tab.under
                .iter()
                .find(|under| matches!(under, Under::Replace { .. })),
            Some(Under::Replace { began: true, .. })
        ));
        taken_until(&mut tab, &sessions, |tab| restores(tab) == 0);
        assert!(!tab.restoring());
        assert_eq!(tab.sent[tab.sent.len() - 2..], [Sent::Begin, Sent::Replace]);
        assert_eq!(tab.resets, resets + 1, "the fields reset once");
        // Asked again, the transaction open: its put-back begins none, so a refusal of it
        // would leave the transaction as it was.
        assert!(tab.transaction);
        tab.restore(&sessions, one);
        taken_until(&mut tab, &sessions, |tab| {
            tab.under
                .iter()
                .any(|under| matches!(under, Under::Replace { .. }))
        });
        assert!(matches!(
            tab.under
                .iter()
                .find(|under| matches!(under, Under::Replace { .. })),
            Some(Under::Replace { began: false, .. })
        ));
        taken_until(&mut tab, &sessions, |tab| restores(tab) == 0);
        assert_eq!(tab.resets, resets + 2, "the fields reset again");
    }

    #[test]
    fn restores_refusal_is_said_in_its_reloads_banner_or_else_in_the_status_bar() {
        // DESIGN.md, 6c: Restore's refusal is said in the banner of the reload that replaced
        // the revision it asked for, while that banner shows; a refusal of another
        // revision's, in the status bar.
        let (one, two, three) = (
            Revision::from_number(1).unwrap(),
            Revision::from_number(2).unwrap(),
            Revision::from_number(3).unwrap(),
        );
        let mut tab = tab();
        tab.reloaded = Some(Reloaded {
            from: two,
            to: three,
            changes: Changes::Asking,
            refused: None,
            unlisted: Vec::new(),
            from_held: true,
        });
        let problems = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "revision 1 isn't held",
        ));
        tab.restore_refused(one, &problems);
        assert_eq!(tab.message.as_deref(), Some("revision 1 isn't held"));
        assert_eq!(tab.reloaded.as_ref().unwrap().refused, None);
        tab.message = None;
        let problems = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::Internal,
            "revision 2 isn't held",
        ));
        tab.restore_refused(two, &problems);
        assert_eq!(
            tab.reloaded.as_ref().unwrap().refused.as_deref(),
            Some("revision 2 isn't held")
        );
        assert_eq!(tab.message, None);
    }

    /// `tab`'s document read (its timeline, as `read_timeline` takes it), then Rebuild
    /// everything sent and answered, what the tab said of the document before cleared.
    fn rebuilt_everything(tab: &mut Tab, sessions: &FakeSessions) {
        let timeline = sessions
            .timeline(&tab.id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        tab.took_timeline(timeline);
        tab.document_problems.clear();
        tab.rebuild_all(sessions);
        assert_eq!(tab.rebuilds_all, 1, "sent");
        taken_until(tab, sessions, |tab| tab.check_read.is_none());
    }

    // ---- A broken reference's repair: made only of the document its card was read of ----

    /// [`BLOCK`]'s block, and Round, a fillet of its Edge1 and Edge13, which no box has:
    /// Edge13 broken, at `edges.1`.
    const BROKEN_ROUND: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge13"], "radius": 0.1}]}"#;

    /// Polled until nothing of the tab's is under way and the session's revision's model is
    /// published, as the selection holds it.
    fn published_quietly(tab: &mut Tab, sessions: &FakeSessions) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            tab.poll(sessions);
            let published = tab.selection.model().map(|(_, revision)| revision);
            if tab.quiet() && published == Some(tab.revision) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "never published");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A card of Round's name at `place`, `name`, read at `read`.
    fn round_card(place: usize, name: &str, read: Revision) -> crate::repair::Broken {
        crate::repair::Broken {
            feature: FeatureId::new("f0000001").unwrap(),
            place,
            name: name.to_owned(),
            candidates: vec!["k7f2qx3m.box.Edge12".to_owned()],
            read,
        }
    }

    /// `tab`'s `use_candidate` of Edge12 for `card` refused as stale: nothing sent.
    fn refused_as_stale(
        tab: &mut Tab,
        sessions: &FakeSessions,
        card: &crate::repair::Broken,
        case: &str,
    ) {
        let sent = tab.sent.clone();
        assert!(
            !tab.use_candidate(sessions, card, "k7f2qx3m.box.Edge12"),
            "{case}: started"
        );
        assert_eq!(tab.sent, sent, "{case}: sent");
        assert_eq!(tab.message.as_deref(), Some(crate::repair::STALE), "{case}");
        assert_eq!(tab.told, None, "{case}");
        assert!(
            !tab.remove_reference(sessions, card),
            "{case}: Remove started"
        );
        assert_eq!(tab.sent, sent, "{case}: Remove sent");
        assert_eq!(
            tab.message.as_deref(),
            Some(crate::repair::STALE),
            "{case}: Remove"
        );
    }

    #[test]
    fn a_repair_with_a_sketch_open_is_refused_saying_why_and_made_once_it_closes() {
        let (_dir, sessions, mut tab) = opened_on(BROKEN_ROUND, |_| {});
        published_quietly(&mut tab, &sessions);
        let card = round_card(1, "k7f2qx3m.box.Edge13", tab.revision);
        // Sketch mode's Cancel undoes to where it opened, so a mend made under it would go
        // with it, as under a dialog: Use and Remove the edge refused, saying why.
        tab.sketch = Some(crate::sketch::SketchMode::new(1, None));
        let sent = tab.sent.clone();
        let why = Some("Finish or cancel the sketch first");
        tab.told = Some("Round now rounds the edge you picked.".to_owned());
        assert!(!tab.use_candidate(&sessions, &card, "k7f2qx3m.box.Edge12"));
        assert_eq!(tab.message.as_deref(), why);
        assert_eq!(tab.told, None);
        tab.message = None;
        assert!(!tab.remove_reference(&sessions, &card));
        assert_eq!(tab.message.as_deref(), why);
        assert_eq!(tab.sent, sent);
        // The sketch closed: Use mends it, at the broken name's place.
        tab.sketch = None;
        assert!(tab.use_candidate(&sessions, &card, "k7f2qx3m.box.Edge12"));
        assert_eq!(tab.sent.last(), Some(&Sent::Edit("edges.1".to_owned())));
    }

    #[test]
    fn a_repair_is_made_only_of_a_card_read_at_the_sessions_revision_with_its_model_and_nothing_under_way()
     {
        use std::sync::atomic::{AtomicBool, Ordering};
        // Every kernel request held while `holding` is set.
        let holding = Arc::new(AtomicBool::new(false));
        let mut hold = None;
        let (_dir, sessions, mut tab) = opened_on(BROKEN_ROUND, |kernel| {
            let holding = Arc::clone(&holding);
            hold = Some(kernel.hold(move |_| holding.load(Ordering::SeqCst)));
        });
        let hold = hold.unwrap();
        published_quietly(&mut tab, &sessions);
        let opened = tab.revision;
        assert_eq!(opened, Revision::OPENED);
        let length = |value: &str| Edit {
            feature: FeatureId::new("k7f2qx3m").unwrap(),
            field: "length".to_owned(),
            value: silico_core::document::json::read(value.as_bytes()).unwrap(),
        };
        // An edit of the tab's under way: the revision it makes isn't known yet.
        tab.edit(&sessions, length("5"));
        assert!(!tab.quiet());
        refused_as_stale(
            &mut tab,
            &sessions,
            &round_card(1, "k7f2qx3m.box.Edge13", opened),
            "an edit under way",
        );
        published_quietly(&mut tab, &sessions);
        let edited = tab.revision;
        assert!(edited > opened);
        // A card read at the revision before: whatever the document holds now.
        refused_as_stale(
            &mut tab,
            &sessions,
            &round_card(1, "k7f2qx3m.box.Edge13", opened),
            "read at the revision before",
        );
        // The session's revision's model not published yet (its rebuild held): the card
        // read at it, but the model the selection reads is the revision before's.
        holding.store(true, Ordering::SeqCst);
        tab.edit(&sessions, length("6"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !(tab.quiet() && tab.revision > edited) {
            tab.poll(&sessions);
            assert!(std::time::Instant::now() < deadline, "never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            tab.selection.model().map(|(_, revision)| revision),
            Some(edited)
        );
        let now = tab.revision;
        refused_as_stale(
            &mut tab,
            &sessions,
            &round_card(1, "k7f2qx3m.box.Edge13", now),
            "its model not published",
        );
        holding.store(false, Ordering::SeqCst);
        hold.release();
        published_quietly(&mut tab, &sessions);
        // A name the model finds (Edge1, at its own place): never written over.
        let now = tab.revision;
        refused_as_stale(
            &mut tab,
            &sessions,
            &round_card(0, "k7f2qx3m.box.Edge1", now),
            "a name found",
        );
        // Read at the session's revision, its model published, nothing under way, its name
        // broken: written at its place.
        let card = round_card(1, "k7f2qx3m.box.Edge13", tab.revision);
        assert!(tab.use_candidate(&sessions, &card, "k7f2qx3m.box.Edge12"));
        assert_eq!(tab.sent.last(), Some(&Sent::Edit("edges.1".to_owned())));
        assert_eq!(
            tab.told.as_deref(),
            Some("Round now rounds the edge you picked. Undo puts the broken reference back.")
        );
    }

    #[test]
    fn rebuild_everything_says_a_features_fault_as_the_features_never_the_documents() {
        // The gate passes the document (its timeline counts no problem of the gate's); the
        // block's build is refused, a problem of the block's: said once the block is opened
        // (selected), as any feature's mark is, and nothing said of the document.
        use silico_core::kernel::{Op, Primitive};
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |kernel| {
            kernel.refuse(|op| {
                matches!(op, Op::MakePrimitive(Primitive::Box { .. })).then(|| {
                    silico_core::Error::new(ErrorCode::KernelFailed, "the block came apart")
                })
            });
        });
        rebuilt_everything(&mut tab, &sessions);
        assert_eq!(tab.timeline.as_ref().map(|t| t.problems), Some(0));
        assert_eq!(
            tab.said.get("k7f2qx3m").map(|said| said.message.as_str()),
            Some("the block came apart")
        );
        assert!(
            tab.document_problems.is_empty(),
            "{:?}",
            tab.document_problems
        );
    }

    #[test]
    fn a_rebuild_everything_check_of_a_revision_a_reload_moved_past_marks_nothing() {
        // The block's build refused: Rebuild everything's check marks it.
        use silico_core::kernel::{Op, Primitive};
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(&document, BLOCK).unwrap();
        let kernel = silico_core::fake::FakeKernel::new(1).unwrap();
        kernel.refuse(|op| {
            matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
                .then(|| silico_core::Error::new(ErrorCode::KernelFailed, "the block came apart"))
        });
        // The session compares its file on disk as often as it may, reading it as it is.
        let read: silico_core::disk::Reader = Arc::new(|path: &std::path::Path| {
            std::fs::read(path).map_or(silico_core::disk::Found::Gone, |bytes| {
                silico_core::disk::Found::Bytes(bytes)
            })
        });
        let now: silico_core::disk::Clock = Arc::new(std::time::Instant::now);
        let sessions: FakeSessions = Arc::new(
            Sessions::with_cache_opener(Arc::new(kernel), |dir| {
                silico_core::cache::Cache::unsealed(dir)
            })
            .watching_disk(std::time::Duration::ZERO, read, now)
            .unwrap(),
        );
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        let timeline = sessions
            .timeline(&id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        let first = timeline.revision;
        tab.took_timeline(timeline);
        // Ctrl+Shift+B, and its check answered, of the first revision, marking the block.
        tab.rebuild_all(&sessions);
        let (gate, read) = tab.check_read.take().unwrap();
        let answered = read.wait();
        assert_eq!(
            answered.model.as_ref().map(|model| model.revision),
            Some(first)
        );
        assert!(
            !answered
                .result
                .as_ref()
                .unwrap()
                .problems
                .errors()
                .is_empty()
        );
        // Before the tab takes it, the file changed on disk, and the session's reload (no
        // transaction open) moves the revision on: nothing the tab sent.
        std::fs::write(
            &document,
            BLOCK.replace(r#""name": "Hole""#, r#""name": "Bore""#),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let timeline = loop {
            let timeline = sessions
                .timeline(&id, ModelChoice::Current)
                .wait()
                .result
                .unwrap();
            if timeline.revision > first {
                break timeline;
            }
            assert!(std::time::Instant::now() < deadline, "never reloaded");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        tab.took_timeline(timeline);
        assert!(!tab.check_superseded);
        // The check of the revision before it marks nothing.
        tab.took_check_answer(gate, answered);
        assert!(tab.said.is_empty(), "{:?}", tab.said);
        assert!(tab.document_problems.is_empty());
        assert!(!tab.fresh_out);
    }

    #[test]
    fn rebuild_everything_says_every_problem_of_a_document_the_gate_refused_as_the_documents() {
        // A cut whose tool names no feature: the gate's one problem, of the cut. Every problem
        // the check finds is the document's, the cut's too, as the property panel shows a
        // refused document's.
        let refused = BLOCK.replace(r#""tool": "a91cw0de""#, r#""tool": "zzzzzzzz""#);
        let (_dir, sessions, mut tab) = opened_on(&refused, |_| {});
        rebuilt_everything(&mut tab, &sessions);
        assert_eq!(tab.timeline.as_ref().map(|t| t.problems), Some(1));
        let cut = tab.said.get("m3tq8v0z").expect("the cut's problem").clone();
        assert_eq!(tab.document_problems, [cut]);
    }

    #[test]
    fn rebuild_everything_is_sent_only_with_the_document_read_no_dialog_no_close_and_no_check_out()
    {
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        let ready = || {
            let mut tab = tab();
            tab.timeline = Some(timeline(
                0,
                Some(Revision::OPENED),
                vec![entry("a0000001", 0, 0)],
            ));
            tab
        };
        assert!(ready().can_rebuild_all());
        // Each condition alone unmet.
        let mut unread = ready();
        unread.timeline = None;
        let mut in_dialog = ready();
        in_dialog.loading = Some(1);
        let mut closing = ready();
        closing
            .edits
            .queue
            .push_back(Queued::Save { then_close: true });
        let mut checking = ready();
        checking.check_read = Some((false, sessions.check(&checking.id, CheckModel::Published)));
        for (what, mut tab) in [
            ("unread", unread),
            ("in a dialog", in_dialog),
            ("closing", closing),
            ("a check out", checking),
        ] {
            assert!(!tab.can_rebuild_all(), "{what}");
            tab.rebuild_all(&sessions);
            assert_eq!(tab.rebuilds_all, 0, "{what}: sent");
            assert!(!tab.fresh_out, "{what}: sent");
        }
        // All met: sent, once; a second waits for its answer.
        let mut tab = ready();
        tab.rebuild_all(&sessions);
        assert_eq!(tab.rebuilds_all, 1);
        assert!(!tab.can_rebuild_all());
        tab.rebuild_all(&sessions);
        assert_eq!(tab.rebuilds_all, 1);
    }

    #[test]
    fn a_rename_opens_where_asked_else_on_the_timeline_and_never_in_a_dialog_or_a_close() {
        use crate::feature_menu::Place;
        let block = FeatureId::new("a0000001").unwrap();
        let cut = FeatureId::new("b0000001").unwrap();
        // A block and the cut that consumed it: the browser lists bodies by the feature that
        // ends their chain, so the cut has a row, and the block none.
        let ready = || {
            let mut tab = tab();
            tab.timeline = Some(timeline(
                0,
                Some(Revision::OPENED),
                vec![entry("a0000001", 0, 0), entry("b0000001", 0, 0)],
            ));
            tab.browser = Browser {
                bodies: vec![Row {
                    id: cut.clone(),
                    name: "b0000001".to_owned(),
                    health: Health::Healthy,
                    marked_by: None,
                }],
                sketches: Vec::new(),
            };
            tab
        };
        // Nothing to rename under an open dialog (its Cancel undoes to where it opened) or in
        // a tab closing (its edits would be dropped), from the timeline or the browser.
        let mut in_dialog = ready();
        in_dialog.loading = Some(1);
        let mut closing = ready();
        closing
            .edits
            .queue
            .push_back(Queued::Save { then_close: true });
        for (what, mut tab) in [("in a dialog", in_dialog), ("closing", closing)] {
            for place in [Place::Timeline, Place::Browser] {
                tab.start_rename(cut.clone(), place);
                assert_eq!(tab.renaming, None, "{what}, at {place:?}");
                assert_eq!(tab.selection().feature(), None, "{what}, at {place:?}");
            }
        }
        // The cut's in its browser row, as asked, the browser scrolled to it; its name typed
        // over.
        let mut tab = ready();
        tab.start_rename(cut.clone(), Place::Browser);
        let renaming = tab.renaming.clone().unwrap();
        assert_eq!(
            (&renaming.feature, renaming.place, renaming.text.as_str()),
            (&cut, Place::Browser, "b0000001")
        );
        assert_eq!(tab.selection().scroll_target(), Some(&cut));
        assert_eq!(tab.timeline_scroll(), None);
        // The block, asked in the browser, which has no row for it: its timeline item's, the
        // timeline scrolled to it.
        let mut tab = ready();
        tab.start_rename(block.clone(), Place::Browser);
        let renaming = tab.renaming.clone().unwrap();
        assert_eq!(
            (&renaming.feature, renaming.place),
            (&block, Place::Timeline)
        );
        assert_eq!(tab.selection().feature(), Some(&block));
        assert_eq!(tab.timeline_scroll(), Some(&block));
    }

    #[test]
    fn with_nothing_published_poll_after_poll_counts_no_model_and_the_first_published_once() {
        // Every request of the kernel held: the rebuild publishes nothing, and the tab's count
        // of the models it has seen published stays 0, poll after poll; once released, the
        // model published counts once, and polls after it count nothing more.
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(&document, BLOCK).unwrap();
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let hold = kernel.hold(|_| true);
        let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        // The first poll reads the timeline, which starts the rebuild.
        tab.poll(&sessions);
        assert!(kernel.wait_running(1, std::time::Duration::from_secs(60)));
        assert_eq!(tab.published_models, 0);
        for _ in 0..5 {
            tab.poll(&sessions);
            assert_eq!(tab.published, None);
            assert_eq!(tab.published_models, 0);
        }
        hold.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while tab.published.is_none() {
            tab.poll(&sessions);
            assert!(std::time::Instant::now() < deadline, "never published");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tab.published_models, 1);
        for _ in 0..5 {
            tab.poll(&sessions);
            assert_eq!(tab.published_models, 1);
        }
    }

    // ---- #351's mutants: sketch mode's own steps counted, its tool, and its name ----

    #[test]
    fn a_refused_undo_in_sketch_mode_gives_its_redo_nothing_to_give_back() {
        // An undo in sketch mode with nothing to undo (no transaction is open): refused, and
        // said; none of the mode's own edits undone, so none for its Redo.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        assert!(!tab.transaction);
        tab.sketch = Some(SketchMode::new(3, None));
        tab.step(&sessions, true);
        assert!(tab.stepping(), "nothing asked");
        taken_until(&mut tab, &sessions, |tab| !tab.stepping());
        assert!(tab.message.is_some(), "the undo wasn't refused");
        assert_eq!(tab.sketch.as_ref().map(|mode| mode.undone), Some(0));
    }

    #[test]
    fn a_tool_chosen_in_sketch_mode_is_the_modes_a_drawing_tool_and_none_outside_it() {
        use silico_core::sketch_tools::{Preview, ToolKind};
        let mut tab = tab();
        // Outside sketch mode: nothing.
        tab.sketch_tool(Some(ToolKind::Line));
        assert!(tab.sketch.is_none());
        tab.sketch = Some(SketchMode::new(1, None));
        tab.sketch_tool(Some(ToolKind::Circle));
        let mode = tab.sketch.as_ref().unwrap();
        assert_eq!(mode.tools.choice(), Some(Choice::Draw(ToolKind::Circle)));
        assert_eq!(*mode.tools.preview(), Preview::default());
        // Esc's: none.
        tab.sketch_tool(None);
        assert_eq!(tab.sketch.as_ref().unwrap().tools.choice(), None);
    }

    #[test]
    fn the_sketch_open_is_named_by_its_own_browser_row_and_sketch_till_its_listed() {
        let row = |id: &str, name: &str| Row {
            id: FeatureId::new(id).unwrap(),
            name: name.to_owned(),
            health: Health::Healthy,
            marked_by: None,
        };
        let mut tab = tab();
        tab.browser.sketches = vec![row("s0000001", "Loose"), row("s0000002", "Tight")];
        // Edit Sketch on the second: its own row's name, never the other's.
        tab.sketch = Some(SketchMode::new(
            1,
            Some(FeatureId::new("s0000002").unwrap()),
        ));
        assert_eq!(tab.sketch_name(), "Tight");
        tab.sketch = Some(SketchMode::new(
            2,
            Some(FeatureId::new("s0000001").unwrap()),
        ));
        assert_eq!(tab.sketch_name(), "Loose");
        // Create Sketch's, still to be added; and one added but not yet listed.
        tab.sketch = Some(SketchMode::new(3, None));
        assert_eq!(tab.sketch_name(), "Sketch");
        tab.sketch = Some(SketchMode::new(
            4,
            Some(FeatureId::new("s0000009").unwrap()),
        ));
        assert_eq!(tab.sketch_name(), "Sketch");
    }
}
