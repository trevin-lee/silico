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
use silico_core::document::json::{Node, Plain};
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
use crate::sketch::{self, Deletion, FrameInput, SketchMode, SketchReads};

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
    /// An edit, and what the step it makes says ([`Tab::edit_words`]), decided as it's asked
    /// (the dialog or sketch it's made in, or what it does), its feature named once the step
    /// is made, from the state it's made from; none for a rename, which its answer names.
    Edit(Edit, Option<Words>),
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
    /// Reload (Trevin's ask of 2026-11-09), after the revert it asked, if any: its read of
    /// the file, then its Rebuild everything ([`Tab::reload`]). In the queue, so an edit
    /// asked after it is sent after the read, and a later revert drops it.
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
    /// at the state it ended at, so what sketch mode undid isn't redone past its edits. A
    /// dialog's carries its serial (`dialog`), so its Cancel lands what it unmade. An OK or a
    /// Finish carries what the one step it makes says (`words`: "Edit Pad", "Add Sketch1").
    Closed {
        ok: bool,
        sketch: bool,
        dialog: Option<u64>,
        words: Option<Words>,
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

/// A new sweep's body: not yet read, or read from the document at a revision (its base,
/// [`base_of`]: its sketch's face's body, or the tip; none if it has none), which the add is
/// sent with only if the session is still at that revision, and read again otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    Unread,
    Read(Option<FeatureId>, Revision),
}

/// What changes the document as it's sent (an edit, an add): the revision it's sent
/// against, the state the session is at then ([`Tab::state_of`]): the state its step is made
/// from; and what that step says, if it's said as it's sent (none for a rename, a deletion,
/// or an add, which their answers name). Once it's applied (it answers, and the session's
/// revision moved past the one it was sent against), the dialogs' groups and the steps past
/// that state are let go (the redo branch the change dropped), and the state it made is kept
/// as a step, with what it says and the state it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sending {
    against: Revision,
    state: Revision,
    step: Option<Words>,
}

/// What a step says, as the Undo and Redo tooltips name it (the prototype's labels). Each
/// kind of step says it from one source: a rename, a deletion, and an add from the session's
/// answer (`set_field`'s `before`, the feature removed, the feature added: [`Words::Text`]);
/// every other one as it's sent, a verb and a feature, or a feature and what was done to it,
/// the feature named by the name it has in the state the step was made from, or, a feature
/// the step made (a dialog's OK'd add), in the state it made ([`Tab::step_text`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Words {
    /// "Suppress Pin", "Move Pin", "Edit Pad", and a dialog's OK'd add, "Add Pad".
    Of(&'static str, FeatureId),
    /// "Inside corner: use the edge you picked".
    Then(FeatureId, &'static str),
    /// "Rename Pin", "Delete Hole", "Add Pad" (from their answers), "Restore revision 4",
    /// "Add Sketch1" (Create Sketch's Finish before its add answers), "Edit".
    Text(String),
}

impl Words {
    /// What a step whose change says nothing more of itself says: the prototype's `edit`'s
    /// own (`opts.label || 'Edit'`).
    fn edit() -> Words {
        Words::Text("Edit".to_owned())
    }
}

/// A step the tab's own changes made, by the state it made: what it says, and the state it
/// was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    words: Words,
    from: Revision,
}

/// The features' names in a state the tab knows them in ([`Tab::names`]): as a timeline of
/// it, or of one before it, listed them (shared between the states that keep them), and the
/// names the tab's own changes gave since (a rename's, an add's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Names {
    listed: Arc<BTreeMap<FeatureId, String>>,
    since: BTreeMap<FeatureId, String>,
}

impl Names {
    /// The feature `feature`'s name in it, if it has one.
    fn get(&self, feature: &FeatureId) -> Option<&str> {
        self.since
            .get(feature)
            .or_else(|| self.listed.get(feature))
            .map(String::as_str)
    }
}

/// What a change does to the names of the state it's made from ([`Tab::applied`]): nothing,
/// or names one feature (a rename, an add); or what it does isn't known here (a Restore's
/// put-back, which replaces the whole text), so the state it makes has none till a timeline
/// of it lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Effect {
    Same,
    Names(FeatureId, String),
    Unknown,
}

/// How many of the commands it sent a tab keeps in its record ([`Tab::sent`]): the newest.
pub const SENT_KEPT: usize = 256;

/// Why a boolean's command is off in a part of fewer than two bodies ([`Tab::why_closed`]).
pub const NEEDS_TWO_BODIES: &str = "Needs two bodies";

/// Why nothing of a document the gate (or the reader) refused changes but its text: a
/// feature's menu's items and the toolbar's dialogs say it.
pub const UNMENDED: &str =
    "The document has problems of its own: no feature changes until they're mended";

/// What Look At says with no flat face to look at ([`Tab::look_at`]): nothing selected,
/// several things, one that isn't a face, or a face the core measured not planar. The
/// prototype's words.
pub const LOOK_AT_NO_FACE: &str = "Look At: select a flat face first.";

/// What Look At says of the one face selected while the core's measures of it haven't come:
/// its `elements` read is out, or, while a dialog is open, not asked until the dialog closes
/// (`Tab::measure`).
pub const LOOK_AT_UNMEASURED: &str = "Look At: the face selected isn't measured yet.";

/// What Look At says of the one face selected when the core's measures of it failed: the
/// selection view says why.
pub const LOOK_AT_UNMEASURABLE: &str = "Look At: the face selected couldn't be measured.";

/// What Look At says in sketch mode before the sketch's first read has come. Once it has, a
/// sketch the rebuild couldn't place (on a face not found) says the read's own error.
pub const LOOK_AT_UNREAD: &str = "Look At: the sketch isn't read yet.";

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

/// Whose the transaction open after a dialog or sketch mode opened is ([`Tab::opens_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// The dialog's, none having been open where it opened (as at any reload): its Cancel, or
    /// its OK with nothing changed, rolls the transaction back.
    Opened,
    /// What came before it: its Cancel undoes back to where it opened, the edits before
    /// staying.
    Before,
}

/// A dialog's edits, OK'd: one step for undo and redo (section 9), from the state it opened
/// at to the state it was OK'd at, each named by the first revision the session was at it
/// ([`Tab::state_of`]): an undo at its end undoes to its start, and a redo at its start
/// redoes to its end. And what the step says, its feature named from its start (or, made
/// by it, its end).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    start: Revision,
    end: Revision,
    words: Words,
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
    /// Whether an edit of `feature`'s `field` is in flight or queued.
    fn pending_field(&self, feature: &FeatureId, field: &str) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|flying| &flying.field.0 == feature && flying.field.1 == field)
            || self.queue.iter().any(|queued| {
                matches!(queued, Queued::Edit(edit, _) if &edit.feature == feature && edit.field == field)
            })
    }

    /// Whether an edit of `feature` is in flight or queued.
    fn pending_for(&self, feature: &FeatureId) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|flying| &flying.field.0 == feature)
            || self
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Edit(edit, _) if &edit.feature == feature))
    }

    /// Whether no edit is in flight or queued.
    pub fn idle(&self) -> bool {
        self.in_flight.is_none()
            && !self
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Edit(..)))
    }

    /// `edit` made, saying `step`: in place of one of the same field that waits last, with
    /// only edits after it (keeping what that one says: the two are one edit, asked where
    /// the first was asked, in its dialog or sketch mode), or at the end.
    fn push(&mut self, edit: Edit, step: Option<Words>) {
        for queued in self.queue.iter_mut().rev() {
            match queued {
                Queued::Edit(waiting, _)
                    if waiting.feature == edit.feature && waiting.field == edit.field =>
                {
                    *waiting = edit;
                    return;
                }
                Queued::Edit(..) => {}
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
        self.queue.push_back(Queued::Edit(edit, step));
    }
}

/// A command a tab has under way, besides its reads and its edits.
enum Under {
    /// A `begin`, sent at the tab's revision `sent_at`: the session reads its file first,
    /// and reloads it if it changed on disk, before the transaction opens.
    Begin {
        sent_at: Revision,
        running: Running<Begun>,
    },
    Commit {
        running: Running<Committed>,
        then_close: bool,
    },
    /// A rollback, and what Cancel or revert it lands ([`Tab::land`]) where it answers.
    Rollback {
        running: Running<RolledBack>,
        then_close: bool,
        lands: Lands,
    },
    /// An undo or a redo, the sketch mode it was asked in, if it was, and whether it's an
    /// over-defining dimension's Cancel (SD7), whose `conflict` is said as overtaken; and
    /// what dialog's Cancel it lands where it answers, if it's one's undo.
    Step {
        running: Running<Stepped>,
        undo: bool,
        sketch: Option<u64>,
        offered: bool,
        lands: Lands,
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
    /// `serial`, sent at the tab's revision `sent_at`: outside a transaction the session reads
    /// its file first, and reloads it if it changed on disk.
    Load {
        serial: u64,
        kind: Kind,
        feature: FeatureId,
        sent_at: Revision,
        running: Running<TextAnswer>,
    },
    /// Reload's read of the file (`text` of the document now), sent at the tab's revision
    /// `sent_at`: what's asked after it waits for it ([`Tab::reload`]); then Rebuild
    /// everything, unheld.
    ReloadRead {
        sent_at: Revision,
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

impl Under {
    /// The state the change under way was sent from, if it's one: what the state it makes
    /// takes its names from ([`Tab::names`]).
    fn made_from(&self) -> Option<Revision> {
        match self {
            Under::Move { sending, .. }
            | Under::Suppress { sending, .. }
            | Under::Remove { sending, .. }
            | Under::Add { sending, .. }
            | Under::AddSketch { sending, .. }
            | Under::SketchEdit { sending, .. }
            | Under::SketchJoin { sending, .. }
            | Under::SketchAdd { sending, .. }
            | Under::SketchCut { sending, .. }
            | Under::DimensionValue { sending, .. }
            | Under::OfferField { sending, .. } => Some(sending.state),
            Under::Begin { .. }
            // Names are never carried across a save (see `Tab::names`).
            | Under::Commit { .. }
            | Under::SaveAs { .. }
            | Under::Rollback { .. }
            | Under::Step { .. }
            | Under::Close(_)
            | Under::Base { .. }
            | Under::Load { .. }
            | Under::ReloadRead { .. }
            | Under::Restore { .. }
            // What a Restore puts back names what it names: nothing of the state it's sent
            // from.
            | Under::Replace { .. } => None,
        }
    }
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
/// chain's); and, a sketch's, the features built from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: FeatureId,
    pub name: String,
    pub health: Health,
    pub marked_by: Option<FeatureId>,
    /// A sketch's consumers (docs/design/m5.0-app.md, decision 29: "a feature of the
    /// timeline that lists it among its `inputs`, built or not, suppressed or not"), as
    /// [`tip_of`] counts a consumed feature, less the one the open dialog is making, which
    /// counts from its OK. None for a body.
    pub consumers: std::collections::BTreeSet<FeatureId>,
    /// Whether its feature is suppressed (a sketch's: then nothing of it is drawn).
    pub suppressed: bool,
}

/// An eye clicked away from its row's default (docs/design/m5.0-app.md, decision 29: "kept
/// until the tab closes or a feature is newly built from the sketch"): shown or hidden,
/// and, a sketch's, the consumers it had when it was set or last confirmed, so a feature
/// newly built from it drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub shown: bool,
    pub under: std::collections::BTreeSet<FeatureId>,
}

/// Where a dialog's Cancel, or a revert, lands: not yet sent, or at the revision its undo
/// or rollback answered at (the session's then, if it sent none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Landing {
    Unsent,
    At(Revision),
}

impl Landing {
    /// Whether a timeline answer at `revision` is from before it lands: every answer is,
    /// till it's sent.
    fn after(self, revision: Revision) -> bool {
        match self {
            Landing::Unsent => true,
            Landing::At(at) => revision < at,
        }
    }
}

/// A new feature whose dialog closed without OK, or was dropped: the feature, the dialog's
/// serial, and where its undo or rollback lands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unmade {
    feature: FeatureId,
    serial: u64,
    landing: Landing,
}

/// What an undo or a rollback lands where it answers ([`Tab::land`]): nothing; the Cancel
/// of the dialog numbered so; or, a revert's, every Cancel and dropped dialog not landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lands {
    Nothing,
    Dialog(u64),
    Everything,
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
    bodies: Vec<Row>,
    sketches: Vec<Row>,
    /// Each sketch row's ID, and whether a feature is built from it: what a sketch's
    /// default is looked up in ([`shown_by_default`]), worked out with the rows.
    consumed: BTreeMap<FeatureId, bool>,
}

impl Browser {
    /// Its bodies' rows, in the timeline's order.
    pub fn bodies(&self) -> &[Row] {
        &self.bodies
    }

    /// Its sketches' rows, in the timeline's order.
    pub fn sketches(&self) -> &[Row] {
        &self.sketches
    }

    /// The browser of `bodies` and `sketches`, each sketch's consumers looked up from them.
    pub fn new(bodies: Vec<Row>, sketches: Vec<Row>) -> Browser {
        let consumed = sketches
            .iter()
            .map(|row| (row.id.clone(), !row.consumers.is_empty()))
            .collect();
        Browser {
            bodies,
            sketches,
            consumed,
        }
    }
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
    /// How many models had been seen published ([`published_models`](Self::published_models))
    /// when the read `timeline` holds was asked: so its model is that one, or a newer one.
    /// What a test reads, to know the timeline is of a model it saw published.
    pub timeline_after: u64,
    timeline_read: Option<Running<TimelineAnswer>>,
    /// `timeline_after` for the read out.
    timeline_read_after: u64,
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
    /// reload of its file (a revision the tab didn't ask for, seen once, by a status read
    /// with nothing of the tab's under way or by an answer it came before). The property
    /// panel's field then shows the document's value again. An edit
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
    /// Reload's Rebuild everything, its read of the file answered, waiting for a check of
    /// the tab's own to answer first (one is out at a time).
    reload_rebuild: bool,
    /// The revision the newest reload the tab saw came to. A reload to it or before it was
    /// seen already, and is never announced again ([`reload_is_new`](Self::reload_is_new)):
    /// two answers sent at one revision (Edit Feature's read and a begin) may each say the
    /// session moved past it, and Edit Feature's read's answer moves the dialog, not the
    /// tab's revision (an answer at the same revision as a begin's would undo the
    /// transaction it says), so the poll after it finds the session past the tab too. A
    /// `status` of an older revision compared the file with the bytes that reload replaced.
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
    /// What the core says of the face the canvas's context menu is over: its entry, read as
    /// the selection view's is, for its type (Sketch on this face, `crate::canvas_menu`).
    menu_face: crate::measures::Measures,
    /// What a toast is to say for the tab (a box's count, or why it took nothing), until
    /// the app takes it.
    toast: Option<String>,
    /// The `diff` of the reload [`reloaded`](Self::reloaded) shows, asked as the reload was
    /// seen, from the revision the tab had to the session's now, until it answers.
    reload_diff: Option<Running<DiffAnswer>>,
    /// The features another program changed, each by its ID with what changed in it, as
    /// the reloads' diffs answered (`d.outside`): marked info-blue in the timeline and the
    /// browser until the person marks it seen in its properties (DESIGN.md, 6c).
    pub outside: BTreeMap<FeatureId, String>,
    /// Show changes (DESIGN.md, 6d): what another program wrote to the file and what the
    /// edits held changed, each as its own `diff` answers ([`HeldChanges`]); none until
    /// it's asked, and again once its modal closes.
    pub held_changes: Option<HeldChanges>,
    /// Show changes' two diffs, the file's and the edits', each until it answers.
    theirs_diff: Option<Running<DiffAnswer>>,
    mine_diff: Option<Running<DiffAnswer>>,
    /// Each browser eye clicked away from its default (docs/design/m5.0-app.md, decision
    /// 29): the tab's state, never saved, dropped when the tab closes, and a sketch's when a
    /// feature is newly built from it. A sketch shows by default while nothing is built
    /// from it; a body and an origin row always do.
    pub overrides: BTreeMap<Hideable, Override>,
    /// Each new feature's dialog cancelled, its feature undone or rolled back, with the
    /// revision that undo or rollback comes to (none till its Cancel is sent): no sketch's
    /// consumer in a timeline answer older than that, so a read answered before the undo
    /// gains no sketch a consumer ("a feature the open dialog is making counts from its
    /// OK", decision 29). Dropped at the first answer at or past it, which lists the
    /// feature only if something since (a redo) made it again.
    unmade: Vec<Unmade>,
    /// An Edit Feature dialog closed without OK (its serial), and where its undo lands: no
    /// override changes for a timeline answer older than that, as none does while the
    /// dialog is open ("Edit Feature … drop nothing", decision 29).
    edit_undone: Option<(u64, Landing)>,
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
    /// The revision the open dialog or sketch mode opened at ([`opens_at`](Self::opens_at)):
    /// where its opening came up in the queue, or the revision of a reload seen since, kept
    /// till its close is taken; and whether the transaction since then is its own, none
    /// having been open there.
    dialog_base: Option<Revision>,
    dialog_began: bool,
    /// Sketch mode (section 9, *Sketching*), while a sketch is open; and each sketch's read,
    /// whole, for the browser's prefixes and the viewport's lines.
    pub sketch: Option<SketchMode>,
    pub sketch_reads: SketchReads,
    /// The shown sketches' lines the viewport draws, each made again only when its read is
    /// replaced or the view's scale at it leaves its bucket (`crate::sketch::lines`).
    pub sketch_lines: crate::sketch::lines::ShownLines,
    /// The selection as it was before the open dialog's box of names took it, put back if
    /// the dialog is cancelled, as SolidWorks returns to the selection its command started
    /// with.
    before_dialog: Option<Vec<crate::selection::Item>>,
    /// The last dialog OK'd, for the window to remember its values ([`take_oked`]
    /// (Self::take_oked), [`crate::dialog::Remembered`]).
    oked: Option<Dialog>,
    /// The dialogs' edits OK'd in this transaction, each one step for undo and redo.
    groups: Vec<Group>,
    /// Each state the tab's changes made in this transaction, by its first revision
    /// ([`state_of`](Self::state_of)), with the step that made it: what the Undo and Redo
    /// tooltips name, and the status bar counts. In order, those up to the session's state its
    /// undo's reach, those after it its redo's; those past the state a change was made at let
    /// go as it's applied (the redo branch it dropped), and all of them as the transaction
    /// ends.
    steps: BTreeMap<Revision, Step>,
    /// The features' names in each state the tab knows them in, by the state's first
    /// revision: from a timeline of that state, or, for a state the tab's own changes make
    /// inside a transaction, from the state each was made from and what it did ([`Effect`]).
    /// Never carried across a save, a Save As, or a rollback: the state each comes to has none
    /// till a timeline lists it (names lent across a reload would be wrong; none is never
    /// wrong). An undo or a redo comes back to a state already here.
    /// What a step says is worked out from it ([`step_text`](Self::step_text)). Kept for:
    /// - the states the steps were made from and made, and the groups' starts and ends;
    /// - the states a change in flight, a drag's end, or a change under way was sent from
    ///   ([`Under::made_from`]), whose made state takes them;
    /// - the session's now.
    names: BTreeMap<Revision, Names>,
    /// The unsaved edits the bar holds while something of the tab's is under way
    /// ([`unsaved_edits`](Self::unsaved_edits)), set three ways: by the last poll that found
    /// nothing under way, as it counted them; by a dialog's or sketch mode's close, as it's
    /// asked, to what it leaves ([`count_closed`](Self::count_closed)); and to none as a save
    /// or a revert ends the transaction ([`stack_gone`](Self::stack_gone)).
    held_count: usize,
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

/// What `dialog`'s step says, `verb` and its feature: by its ID once it has one (a new
/// feature's, once its add has answered); till then, by its kind's title ([`Kind::title`]),
/// the name its add gives it.
fn dialog_words(dialog: &Dialog, verb: &'static str) -> Words {
    match &dialog.feature {
        Some(feature) => Words::Of(verb, feature.clone()),
        None => Words::Text(format!("{verb} {}", dialog.kind.title())),
    }
}

/// What an add's answer says of the step it made, `feature`: "Add" and its name, and the
/// name it gives the state it made.
fn made_by(feature: &silico_core::document::Feature) -> (Words, Effect) {
    let name = feature.name.as_str();
    (
        Words::Text(format!("Add {}", step_name(name))),
        Effect::Names(feature.id.clone(), name.to_owned()),
    )
}

/// What a rename's answer says of the step it made, and does to the names of the state it
/// made from: "Rename" and the name it replaced, as the session held it there (`before`),
/// and the feature named as it set it (`after`). None for an edit of another field, or of
/// a value that isn't a name.
fn renaming(set: &FieldSet) -> Option<(Words, Effect)> {
    match (set.field.as_str(), &set.before, &set.after) {
        ("name", Plain::String(before), Plain::String(after)) => Some((
            Words::Text(format!("Rename {}", step_name(before))),
            Effect::Names(set.feature.clone(), after.clone()),
        )),
        _ => None,
    }
}

/// A feature's name as a step's words give it ("Rename" and it), as the Undo and Redo
/// tooltips show them: on one line, each run of white space one space and none at its ends,
/// as the prototype's tooltip, HTML, lays out a line break in a name; and cut short at
/// [`changes::SHOWN_CHARS`] characters with an ellipsis, as the reload banner's rows cut a
/// long name, so a name wider than the window keeps the tooltip on it.
fn step_name(name: &str) -> String {
    changes::shown(&name.split_whitespace().collect::<Vec<_>>().join(" "))
}

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
            timeline_after: 0,
            timeline_read: None,
            timeline_read_after: 0,
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
            reload_rebuild: false,
            reloaded_to: None,
            selection: crate::selection::Selection::default(),
            box_select: crate::box_select::BoxSelect::default(),
            select_other: crate::select_other::SelectOther::default(),
            measures: crate::measures::Measures::default(),
            menu_face: crate::measures::Measures::default(),
            toast: None,
            reload_diff: None,
            outside: BTreeMap::new(),
            held_changes: None,
            theirs_diff: None,
            mine_diff: None,
            overrides: BTreeMap::new(),
            unmade: Vec::new(),
            edit_undone: None,
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
            sketch_reads: SketchReads::default(),
            sketch_lines: Default::default(),
            before_dialog: None,
            oked: None,
            groups: Vec::new(),
            steps: BTreeMap::new(),
            names: BTreeMap::new(),
            held_count: 0,
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
            sketch_off: crate::feature_menu::sketch_off(self),
        };
        let (left, asked) = self.select_other.overlay(
            &mut self.selection,
            &mut self.view,
            frame,
            painter,
            current,
            &facts,
        );
        self.menu_face_read(sessions, painter.ctx());
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
        let select_other = &self.select_other;
        self.selection.overlay(
            &mut self.view,
            frame,
            painter.ctx(),
            &|view, frame, pixel| select_other.face_under(view, frame, pixel),
        );
        let hidden: std::collections::BTreeSet<silico_core::document::FeatureId> = self
            .browser
            .bodies
            .iter()
            .chain(self.browser.sketches())
            .filter(|row| !self.shown(&Hideable::Feature(row.id.clone())))
            .map(|row| row.id.clone())
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
    /// feature menus' Edit feature (`crate::feature_menu::act`), Sketch on this face as the
    /// selection view's button ([`create_sketch_on_face`](Self::create_sketch_on_face)),
    /// Look At as Ctrl+8 turns to the face selected ([`look_at_face`](Self::look_at_face)),
    /// Hide as H hides a body, Fit and Home view as F6 and Home, Clear selection as Esc
    /// clears it, and a plane of Create Sketch's as the toolbar's
    /// ([`create_sketch`](Self::create_sketch)).
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
            CanvasAct::SketchOnFace(face) => {
                self.create_sketch_on_face(sessions, &face.to_string());
            }
            CanvasAct::LookAt(toward) => self.look_at_face(toward),
            CanvasAct::Hide(id) => self.toggle_shown(Hideable::Feature(id)),
            CanvasAct::Fit => self.view.ask(ViewAsk::Fit),
            CanvasAct::Home => self.view.ask(ViewAsk::Named(crate::navigate::HOME)),
            CanvasAct::ClearSelection => self.selection.clear(),
            CanvasAct::CreateSketch(plane) => self.create_sketch(sessions, plane),
        }
    }

    /// What sketch mode adds to the view's fits this frame ([`crate::navigate::Framing`]):
    /// the open sketch's geometry, and `inset`, the bands its bar and palette cover, in the
    /// canvas's pixels; nothing outside sketch mode.
    pub fn frame_view(&mut self, inset: crate::navigate::Inset) {
        let framing = self
            .sketch
            .as_ref()
            .map_or_else(crate::navigate::Framing::default, |mode| {
                crate::navigate::Framing {
                    sketch_box: mode.model_box(),
                    inset,
                }
            });
        self.view.frame_with(framing);
    }

    /// Look At (Ctrl+8; Fusion's Look At, SolidWorks' Normal To; the prototype's `lookAt`):
    /// in sketch mode, the view square to the sketch's plane, its x across and its y up;
    /// otherwise square to the one face selected, if the core measured it planar, from its
    /// outward normal, up as the named views have it ([`crate::navigate::up_across`]). Each
    /// fits what a named view fits. Else what's missing is said, and the view stays.
    pub fn look_at(&mut self) {
        use crate::viewport::ViewAsk;
        let asked = match self.sketch.as_ref() {
            Some(mode) => match mode.drawable().map(|read| &read.plane) {
                Some(Ok(plane)) => Ok((plane.normal(), plane.y_axis())),
                Some(Err(error)) => Err(format!("Look At: {}", error.message())),
                None => Err(LOOK_AT_UNREAD.to_owned()),
            },
            None => match self.selected_plane_normal() {
                Ok(toward) => return self.look_at_face(toward),
                Err(said) => Err(said.to_owned()),
            },
        };
        match asked {
            Ok((toward, up)) => self.view.ask(ViewAsk::LookAt { toward, up }),
            Err(said) => self.toast = Some(said),
        }
    }

    /// The view square to a planar face from `toward`, its outward unit normal, up as the
    /// named views have it ([`crate::navigate::up_across`]): Look At's turn to a face, from
    /// the face selected (Ctrl+8) or the one under the canvas menu.
    pub fn look_at_face(&mut self, toward: [f64; 3]) {
        self.view.ask(crate::viewport::ViewAsk::LookAt {
            toward,
            up: crate::navigate::up_across(toward),
        });
    }

    /// The outward normal of the one element selected, if it's a face the core's answer for
    /// the selection as it is now says is planar (only a planar face's entry has a normal);
    /// else what Look At says instead.
    fn selected_plane_normal(&self) -> Result<[f64; 3], &'static str> {
        let mut elements = self.selection.elements();
        let (Some(one), None) = (elements.next(), elements.next()) else {
            return Err(LOOK_AT_NO_FACE);
        };
        match one.found().map(silico_core::selection::Selected::kind) {
            None => return Err(LOOK_AT_UNMEASURED),
            Some(silico_core::kernel::ElementKind::Face) => {}
            Some(_) => return Err(LOOK_AT_NO_FACE),
        }
        let answered = crate::measures::Of::now(&self.selection)
            .and_then(|of| self.measures.answers(of))
            .and_then(|answers| answers.element.as_ref());
        match answered {
            Some(Ok(entry)) => entry.normal.ok_or(LOOK_AT_NO_FACE),
            Some(Err(_)) => Err(LOOK_AT_UNMEASURABLE),
            None => Err(LOOK_AT_UNMEASURED),
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

    /// The face the canvas's context menu is over, as read for its type
    /// ([`menu_face_read`](Self::menu_face_read)): of nothing once its answer is taken, or
    /// the menu closes or isn't over a face.
    pub fn menu_face(&self) -> &crate::measures::Measures {
        &self.menu_face
    }

    /// The face the canvas's context menu is over read for its type, as the selection
    /// view's face is (`elements`, through `crate::measures`): asked as the menu opens over
    /// it, polled, and its answer given the menu, whose Sketch on this face and Look At are
    /// there only for a plane; the read let go once its answer is taken, or the menu closes or isn't
    /// over a face. Never waited on.
    fn menu_face_read<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        ctx: &eframe::egui::Context,
    ) {
        let wanted = self.select_other.face_wanted();
        match &wanted {
            Some((of, measured)) => {
                self.menu_face
                    .ask(sessions, &self.id, Some(*of), Some(measured.clone()));
            }
            // Its answer taken, or no menu over a face: its read let go (`ask` does nothing
            // when nothing changed).
            None => self.menu_face.ask(sessions, &self.id, None, None),
        }
        self.menu_face.poll(sessions, &self.id);
        let Some((of, _)) = wanted else {
            return;
        };
        // A planar face's normal, which `elements` gives a planar face alone; none for
        // another face, or a read refused.
        let typed = self
            .menu_face
            .answers(of)
            .and_then(|answers| answers.element.as_ref())
            .map(|element| element.as_ref().ok().and_then(|entry| entry.normal));
        if let Some(normal) = typed {
            self.select_other.face_typed(of, normal, ctx);
        }
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

    /// The step Undo would take back, as its tooltip names it after "Undo: " (the prototype's
    /// `data-tip`, `'Undo: ' + label`): a dialog's OK'd edits by the dialog's words, the
    /// rest each by its own. None while Undo is off, or anything of the tab's is still to
    /// answer (its answer moves what Undo would take back), or the tab doesn't know what made
    /// the state the session is at, or the name its feature had ([`step_text`](Self::step_text)).
    pub fn undo_step(&self) -> Option<String> {
        if !self.undoable() || !self.quiet() {
            return None;
        }
        let at = self.state_of(self.revision);
        // Sketch mode's undo is a step of its own edits, never a dialog's group.
        let group = self
            .groups
            .iter()
            .find(|group| self.sketch.is_none() && group.end == at);
        match group {
            Some(group) => self.step_text(&group.words, group.start, group.end),
            None => {
                let step = self.steps.get(&at)?;
                self.step_text(&step.words, step.from, at)
            }
        }
    }

    /// The step Redo would give back, as its tooltip names it after "Redo: ": a dialog's
    /// OK'd edits, undone, by the dialog's words; else the state after the session's, which
    /// a redo restores, by the step that made it. None while Redo is off, anything of the
    /// tab's is still to answer, or the tab doesn't know what made that state.
    pub fn redo_step(&self) -> Option<String> {
        if !self.redoable() || !self.quiet() {
            return None;
        }
        let at = self.state_of(self.revision);
        let group = self
            .groups
            .iter()
            .find(|group| self.sketch.is_none() && group.start == at);
        match group {
            Some(group) => self.step_text(&group.words, group.start, group.end),
            None => {
                let (made, step) = self.steps.range(at..).find(|(state, _)| **state != at)?;
                self.step_text(&step.words, step.from, *made)
            }
        }
    }

    /// What a step says, made from the state `from` and making `made` (a group: its start
    /// and its end), its feature named as it's named in `from`, or, not there (the step made
    /// it) or not known there, in `made`, cut short as [`step_name`] cuts a name. None while
    /// neither names it: Undo and Redo then say their word alone, never a name of another
    /// state's.
    fn step_text(&self, words: &Words, from: Revision, made: Revision) -> Option<String> {
        let name = |feature: &FeatureId| {
            [from, made]
                .iter()
                .find_map(|state| self.names.get(state)?.get(feature))
                .map(step_name)
        };
        match words {
            Words::Of(verb, feature) => name(feature).map(|name| format!("{verb} {name}")),
            Words::Then(feature, what) => name(feature).map(|name| format!("{name}: {what}")),
            Words::Text(text) => Some(text.clone()),
        }
    }

    /// How many unsaved edits the status bar counts (the prototype's `d.undo.length`): the
    /// steps Undo takes back to where the transaction began, a dialog's OK'd edits one step
    /// as Undo takes them, and the edits of sketch mode open one step as its Finish will make
    /// them. Counted from the steps the tab made, so those the undo stack's bound put out of
    /// reach count still: they're unsaved all the same. While anything of the tab's is under
    /// way, the count held ([`held_count`](Self::held_count)), as the last poll that found
    /// nothing under way counted it, or a dialog's or sketch mode's close asked since left it
    /// ([`count_closed`](Self::count_closed)), or none once a save or a revert ended the
    /// transaction ([`stack_gone`](Self::stack_gone)): the session's revision, read from
    /// `status`, can be past the steps the answers not yet taken will name.
    pub fn unsaved_edits(&self) -> usize {
        if self.quiet() {
            self.edits_now()
        } else {
            self.held_count
        }
    }

    /// [`unsaved_edits`](Self::unsaved_edits) worked out now, nothing of the tab's under way:
    /// the edits of sketch mode or a dialog open one step, from where it opened (its own
    /// opening's: `opened_at`, `dialog_base`).
    fn edits_now(&self) -> usize {
        let at = self.state_of(self.revision);
        // At a state no change of the tab's made (where the transaction began, or one another
        // client of the session moved it to), it knows of none to count.
        if !self.steps.contains_key(&at) {
            return 0;
        }
        let open = match &self.sketch {
            Some(mode) => mode.opened_at,
            None if self.in_dialog() => self.dialog_base,
            None => None,
        };
        self.count_to(at, open.map(|base| self.state_of(base)))
    }

    /// The steps from where the transaction began to the state `at`, a dialog's OK'd edits
    /// one step, and, from the state `open` (sketch mode's or a dialog's, open) to `at`, its
    /// edits one step.
    fn count_to(&self, at: Revision, open: Option<Revision>) -> usize {
        let between = |start: Revision, end: Revision| {
            self.steps
                .range(start..=end)
                .filter(|(state, _)| **state != start)
                .count()
        };
        let mut count = self.steps.range(..=at).count();
        for group in self.groups.iter().filter(|group| group.end <= at) {
            count = count.saturating_sub(between(group.start, group.end).saturating_sub(1));
        }
        if let Some(base) = open.filter(|base| *base < at) {
            count = count.saturating_sub(between(base, at).saturating_sub(1));
        }
        count
    }

    /// The count a dialog's or sketch mode's close leaves, asked now with what it opened at,
    /// `base` (its own opening's), kept while the close and what it sends wait
    /// ([`held_count`](Self::held_count)): the edits before it, and, for an OK or a Finish that
    /// made anything (the session moved past `base`, or an edit of its is still to answer),
    /// its one step; a Cancel's none of its edits, which it takes back.
    fn count_closed(&mut self, ok: bool, base: Revision) {
        let base = self.state_of(base);
        let made =
            ok && (self.state_of(self.revision) != base || !self.edits.idle() || self.changing());
        self.held_count = self.count_to(base, None) + usize::from(made);
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
            .and_then(|id| self.browser.sketches().iter().find(|row| &row.id == id))
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
            Queued::Edit(..)
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
            Under::Begin { .. }
            | Under::Commit { .. }
            | Under::Rollback { .. }
            | Under::Step { .. }
            | Under::SaveAs { .. }
            | Under::Close(_)
            | Under::Load { .. }
            | Under::ReloadRead { .. }
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
        let mut reloaded = None;
        if let Some(session) = &status.session {
            // Moved with nothing of the tab's under way and no transaction open: the
            // session's own reload. A session never reloads inside a transaction, so a move
            // inside one is another client's edit, which a dialog's or sketch mode's Cancel
            // undoes with its own (it opened where it did).
            if self.status.is_some()
                && session.revision != self.revision
                && !session.transaction
                && self.quiet()
                && self.reload_is_new(session.revision)
            {
                reloaded = Some(session.revision);
                self.announce_reload(sessions, self.revision, session.revision);
            }
            self.revision = session.revision;
            self.transaction = session.transaction;
            self.status_revision = session.revision;
        }
        match status.result {
            Ok(status) => {
                // The file as the session holds it, nothing of the tab's under way (a Reload
                // queued or reading is): no reload of the person's is still to come.
                if self.quiet() && !matches!(status.disk, Some(Disk::Changed | Disk::Gone)) {
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
        if let Some(to) = reloaded {
            self.reloaded_under_dialog(to);
        }
        let published = sessions.published(&self.id);
        self.count_published(&published);
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
        // The unsaved edits, counted while every answer of the tab's is taken: what the bar
        // holds while the next is under way.
        if self.quiet() {
            self.held_count = self.edits_now();
        }
    }

    /// The sketch reads polled and started, without waiting: sketch mode's, of its sketch,
    /// and each sketch's whole ([`SketchReads`], the browser's prefixes' and the viewport's
    /// lines'), one sketch at a time, the shown first; each of the newest published model,
    /// read again once that moves.
    fn read_sketches<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(mode) = self.sketch.as_mut() {
            mode.poll(sessions, &self.id, self.published);
            // A drag ended at the newest read's revision or before is drawn by the read.
            if let Some(drag) = self.drag.as_mut() {
                drag.read(mode.read.as_ref().and_then(|read| read.model));
            }
        }
        let (overrides, browser) = (&self.overrides, &self.browser);
        self.sketch_reads.poll(
            sessions,
            &self.id,
            self.browser.sketches(),
            self.published,
            self.sketch.as_ref(),
            &|id| shown_in(overrides, browser, &Hideable::Feature(id.clone())),
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
    /// those it carried. Only for a reload not seen yet
    /// ([`reload_is_new`](Self::reload_is_new)).
    fn announce_reload<P: Clients + 'static>(
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

    /// Whether a reload to `to` is one the tab hasn't seen: none to it or past it seen yet
    /// ([`reloaded_to`](Self::reloaded_to)).
    fn reload_is_new(&self, to: Revision) -> bool {
        self.reloaded_to.is_none_or(|seen| to > seen)
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
        if (session.reloaded || session.revision != sent_at) && self.reload_is_new(session.revision)
        {
            self.announce_reload(sessions, sent_at, session.revision);
            self.reloaded_under_dialog(session.revision);
        }
    }

    /// The session's own reload, once a commit, a Save As, or a rollback ended the
    /// transaction, seen by `status` before that command's answer, `session`, was taken: the
    /// answer's revision behind the tab's, and no transaction open, as it answered nor as
    /// `status` read it (a session never reloads inside one: a refused commit's transaction
    /// stays open, and a move past it, or past a commit into a transaction begun since, is
    /// another client's edit). Nothing of the tab's runs while one of them does, so nothing
    /// else of its moves it. Seen as [`poll`](Self::poll) sees one, which it can't while the
    /// command is under way.
    fn reload_after_end<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: Option<&SessionInfo>,
    ) {
        if let Some(session) = session
            && !session.transaction
            && !self.transaction
            && session.revision < self.revision
            && self.reload_is_new(self.revision)
        {
            self.announce_reload(sessions, session.revision, self.revision);
            self.reloaded_under_dialog(self.revision);
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
                    let reached = response.model.is_some();
                    match response.result {
                        Ok(timeline) => {
                            self.timeline_after = self.timeline_read_after;
                            self.took_timeline(timeline);
                        }
                        // Refused before it reached a model though one is published (too
                        // many published reads running at once, the session closing, no
                        // thread to be had): nothing of it was read, so a later frame asks
                        // again, as the sketch reads do.
                        Err(_) if !reached && self.published.is_some() => {
                            self.timeline_asked = None;
                        }
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
        // Reload's Rebuild everything, once the check out before it has answered; none if
        // it may not be sent now (a dialog opened meanwhile, or the tab closes).
        if self.reload_rebuild && self.check_read.is_none() {
            self.reload_rebuild = false;
            self.rebuild_all(sessions);
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
                lands: Lands::Nothing,
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
        self.took_names(&timeline);
        self.timeline = Some(timeline);
        self.rebrowse();
    }

    /// `under` polled: none if it answered (what it changed taken), or it back.
    fn take<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        under: Under,
        close: &mut bool,
    ) -> Option<Under> {
        match under {
            Under::Begin { sent_at, running } => match running.try_take() {
                Ok(response) => {
                    self.beginning = false;
                    // The session's reload of its file, changed on disk, before the
                    // transaction opened, which nothing said while the begin ran: seen as a
                    // poll sees one.
                    self.reload_in(sessions, sent_at, response.session.as_ref());
                    self.took_session(&response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::Begin { sent_at, running }),
            },
            Under::Commit {
                running,
                then_close,
            } => match running.try_take() {
                Ok(response) => {
                    self.reload_after_end(sessions, response.session.as_ref());
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
                    self.reload_after_end(sessions, response.session.as_ref());
                    self.took_session(&response);
                    match response.result {
                        // The session holds the new file from now on, by the path given: its
                        // transaction ended as a save's is.
                        Ok(committed) => {
                            self.path = Some(committed.path);
                            self.ended(Ok(()), then_close, close);
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
                lands,
            } => match running.try_take() {
                Ok(response) => {
                    self.landed(lands, response.session.as_ref());
                    self.reload_after_end(sessions, response.session.as_ref());
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
                    lands,
                }),
            },
            Under::Step {
                running,
                undo,
                sketch,
                offered,
                lands,
            } => match running.try_take() {
                Ok(response) => {
                    self.landed(lands, response.session.as_ref());
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
                    lands,
                }),
            },
            Under::Add {
                serial,
                sending,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.took_added(&sending, &response);
                    match response.result {
                        Ok(added) => self.add_answered(serial, added.feature.id),
                        Err(problems) => self.add_refused(serial, &problems),
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                    // "Delete" and the feature's name, as the session held it.
                    let words = response.result.as_ref().ok().map(|removed| {
                        Words::Text(format!(
                            "Delete {}",
                            step_name(removed.feature.name.as_str())
                        ))
                    });
                    self.applied_as(&sending, &response, words, Effect::Same);
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
                    self.base_read(serial, kind, members, response.result);
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
                    self.took_added(&sending, &response);
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
                                    dialog: None,
                                    words: None,
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                    self.applied(&sending, &response);
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
                sent_at,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    // The session's reload of its file before the read (the read's own, or one
                    // queued ahead of it), which nothing said while it was out: seen as a poll
                    // sees one. Never inside a transaction, where a move is an edit's.
                    if response.session.as_ref().is_some_and(|s| !s.transaction) {
                        self.reload_in(sessions, sent_at, response.session.as_ref());
                    }
                    if self.loading == Some(serial) {
                        self.loading = None;
                        // The session reloaded its file between the dialog's opening and its
                        // read: it opens at the revision it read, nothing of the tab's having
                        // been sent in between. A move inside a transaction is no reload: it's
                        // what was sent after the dialog opened (an edit typed into it before a
                        // reload had it read again, applied after the reload), its own.
                        if let Some(session) = &response.session
                            && !session.transaction
                            && self
                                .dialog_base
                                .is_some_and(|base| base != session.revision)
                        {
                            self.opens_at(session.revision, Owner::Opened, None);
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
                    sent_at,
                    running,
                }),
            },
            Under::ReloadRead { sent_at, running } => match running.try_take() {
                Ok(response) => {
                    // A reload it made (or one queued ahead of it) is seen as the person's.
                    // One it didn't make leaves the mark to the poll, which lets it go once
                    // nothing of the tab's is under way and the file is as the session holds
                    // it.
                    self.reload_in(sessions, sent_at, response.session.as_ref());
                    self.took_session(&response);
                    match response.result {
                        // Then Rebuild everything, unheld, as ⇧⌘B sends it: an edit after it
                        // supersedes it. A check of the tab's own out answers first.
                        Ok(_) => self.reload_rebuild = true,
                        // The file couldn't be read (gone since the last status, say): said,
                        // nothing rebuilt, and no reload of the person's to come, the poll
                        // never letting the mark go while the file is gone.
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            self.reload_asked = false;
                        }
                    }
                    None
                }
                Err(running) => Some(Under::ReloadRead { sent_at, running }),
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
                    // The text put back names what it names: known once a timeline of it
                    // lists it.
                    self.applied_as(&sending, &response, None, Effect::Unknown);
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
                            // Refused at another revision than it was sent against, the
                            // session having reloaded its file first, its `begin`'s read of
                            // it: no reload said here, that begin's answer having said it.

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

    /// The transaction ended (a save, a Save As, a revert): its undo stack is gone, and with
    /// it the states it held, the dialogs' groups on them, their steps, and their names, but
    /// any a timeline listed for exactly the revision it came to; and the unsaved edits it
    /// counted.
    fn stack_gone(&mut self) {
        self.groups.clear();
        self.steps.clear();
        self.restored.clear();
        // Names are never carried across it: the state it came to has none till a timeline
        // lists exactly that revision (one may have, before its answer was taken).
        let revision = self.revision;
        self.names.retain(|state, _| *state == revision);
        self.redo_fence = None;
        // Its edits are saved, or gone: none of them is counted while what follows waits.
        self.held_count = 0;
    }

    /// A save's, a Save As's, or a revert's answer: the transaction ended, then closed if
    /// `then_close`; or refused, said, and, if it was to close, the tab kept and that said
    /// too. A Save As's refusal is its own to say (it may ask to replace a file, or save
    /// again), so it comes here only answered.
    fn ended(&mut self, result: Result<(), Problems>, then_close: bool, close: &mut bool) {
        match result {
            Ok(()) => {
                self.disk_choice = DiskChoice::None;
                self.stack_gone();
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
                    // A rename says the name it replaced, as the session answers it, and
                    // names the state it made with the one it gave.
                    match response.result.as_ref().ok().and_then(renaming) {
                        Some((words, effect)) => {
                            self.applied_as(&sending, &response, Some(words), effect);
                        }
                        None => self.applied(&sending, &response),
                    }
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
            // What its step says, if it changes the document and says it as it's sent (its
            // feature named from the state the step is made from, once it's made); none for
            // what its answer names, or what changes nothing.
            let step = self.step_words(&queued);
            match queued {
                Queued::Edit(edit, _) => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.edits.sent += 1;
                    self.record(Sent::Edit(edit.field.clone()));
                    let field = (edit.feature.clone(), edit.field.clone());
                    let value = edit.value.clone();
                    let sending = self.sending(step);
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
                    if let Some(dialog) = self
                        .dialog
                        .as_mut()
                        .filter(|dialog| dialog.serial == serial && kind.sweeps())
                    {
                        dialog.base.clone_from(&base);
                    }
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
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
                    self.under.push(Under::Remove {
                        feature: feature.clone(),
                        sending,
                        running: sessions.remove_feature(&self.id, self.revision, feature),
                    });
                }
                Queued::Reload => {
                    // First the file read (`text` of the document now reads it first, outside
                    // a transaction): what's asked after it waits for that alone. The reload
                    // it makes is the person's, whatever a Reload before it came to (an
                    // untitled part's reads no file).
                    self.reload_asked = !self.is_untitled();
                    // The tab's own reads that wait for a rebuild go first, as an edit's
                    // send lets them go: the read would wait behind them, and what's asked
                    // after it with it. Reload's Rebuild everything takes their place.
                    self.supersede_preview();
                    self.under.push(Under::ReloadRead {
                        sent_at: self.revision,
                        running: sessions.text(&self.id, None),
                    });
                }
                Queued::Opened { serial } => {
                    let owner = if self.transaction {
                        Owner::Before
                    } else {
                        Owner::Opened
                    };
                    // Sketch mode's own opening, never another's: what its undo stops at.
                    self.opens_at(self.revision, owner, Some(serial));
                }
                Queued::AddSketch { serial, feature } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Add);
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
                    let running = sessions.sketch_remove(&self.id, self.revision, sketch, members);
                    self.under.push(Under::SketchEdit { sending, running });
                }
                Queued::SketchJoin { sketch, point } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_join"));
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
                    let running =
                        sessions.sketch_trim(&self.id, self.revision, sketch, curve, pick, None);
                    self.under.push(Under::SketchCut { sending, running });
                }
                Queued::SketchSplit { sketch, curve, at } => {
                    self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::SketchEdit("sketch_split"));
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
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
                    let sending = self.sending(step);
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
                                step,
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
                                lands: Lands::Nothing,
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
                            sent_at: self.revision,
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
                    let began = self.begin_if_none(sessions);
                    self.supersede_preview();
                    self.record(Sent::Replace);
                    let sending = self.sending(step);
                    self.under.push(Under::Replace {
                        revision,
                        began,
                        sending,
                        running: sessions.replace_text(&self.id, self.revision, text),
                    });
                }
                Queued::Closed {
                    ok,
                    sketch,
                    dialog,
                    words,
                } => {
                    // Every edit sent before it has answered: what a Cancel put back of the
                    // selection is read from the revision its undo or rollback comes to,
                    // past this one, or, if nothing of the dialog's changed the document,
                    // from this one.
                    let undone = self.revision.next().unwrap_or(self.revision);
                    // What a dialog's Cancel lands (its unmade feature, an Edit Feature's
                    // freeze): where its undo or rollback answers, or here if it sends none.
                    let lands = match dialog {
                        Some(serial) if !ok => Lands::Dialog(serial),
                        _ => Lands::Nothing,
                    };
                    let Some(base) = self.dialog_base.take() else {
                        self.read_restored_from(ok, self.revision);
                        self.land(lands, self.revision);
                        continue;
                    };
                    if !self.transaction {
                        // Nothing of the dialog's changed the document.
                        self.read_restored_from(ok, self.revision);
                        self.land(lands, self.revision);
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
                            lands,
                        });
                    } else if unchanged {
                        // Nothing of the dialog's changed the document.
                        self.read_restored_from(ok, self.revision);
                        self.land(lands, self.revision);
                    } else if ok {
                        self.groups.push(Group {
                            start: self.state_of(base),
                            end: self.state_of(self.revision),
                            words: words.unwrap_or_else(Words::edit),
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
                            lands,
                        });
                    }
                }
                Queued::Save { then_close } if !self.transaction => {
                    if then_close {
                        self.under.push(Under::Close(sessions.close(&self.id)));
                        self.edits.queue.clear();
                    }
                }
                Queued::Revert { then_close } if !self.transaction => {
                    self.land(Lands::Everything, self.revision);
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
                        lands: Lands::Everything,
                    });
                }
                Queued::SaveAs {
                    path,
                    replace,
                    then_close,
                } => {
                    // A transaction begun first if none is open: a Save As commits one,
                    // and one with nothing changed writes the document all the same.
                    let began = self.begin_if_none(sessions);
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
                        lands: Lands::Nothing,
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
    /// its body's read) running; or a `begin` whose answer isn't taken yet, which may say the
    /// session reloaded its file first, and so where what comes next (a dialog's close)
    /// starts from, whatever answers after it were taken first. So no queued item is ever
    /// taken while a `begin` is out: in [`advance`](Self::advance)'s arms `beginning` is
    /// false.
    fn blocked(&self) -> bool {
        self.edits.in_flight.is_some()
            || self.beginning
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
                        | Under::ReloadRead { .. }
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
        self.edits.in_flight.is_none()
            && self.edits.queue.is_empty()
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
        let step = self.edit_words(&edit);
        self.edit_with_words(sessions, edit, step);
    }

    /// [`edit`](Self::edit), the step it makes named `step`.
    fn edit_with_words<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        edit: Edit,
        step: Option<Words>,
    ) {
        self.hush();
        self.edits.push(edit, step);
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
            .chain(self.browser.sketches())
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
        let dropped = self.dialog.take();
        self.dropped(dropped.as_ref());
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
        self.timeline_read_after = self.published_models;
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
    /// reload is the person's, so no banner says another program's. The read is queued
    /// behind the revert ([`Queued::Reload`]), and what's asked after it waits for the read
    /// alone ([`Under::ReloadRead`]); the rebuild is then sent unheld, as ⇧⌘B's, which an
    /// edit after it supersedes.
    ///
    /// An untitled part has no file: it's only rebuilt, its edits kept. On a file gone from
    /// disk nothing is reloaded or dropped: the part open here is the only copy of the work,
    /// so that's said, with Save As to keep it (Main's ruling of 2026-11-09; the core refuses
    /// a read of a gone file outside a transaction, so nothing can be rebuilt). The app asks
    /// it only of a tab that isn't closing and has no dialog open, as Rebuild everything
    /// does nothing in one (it says why); its rebuild is sent once the document is read.
    pub fn reload<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
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

    /// Gives Esc to what takes it before the prototype's `escChain` (its `keydown` gives it
    /// to an open menu, a modal, and the Modify box first), if one is open, and acts on it:
    /// a sketch drag held ends from its begin state (section 3's first, before anything
    /// else); sketch mode's canvas menu closes; the over-define prompt is cancelled (its
    /// Cancel, the prototype's key), the edit undone; a placed dimension's Modify box
    /// closes, its value as it was. Whether one took it. The one list of them:
    /// [`escape`](Self::escape) begins with it, and the app gives Esc to it before it lets
    /// the navigation bar's tool, the chain's first, go.
    pub fn escape_first<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        if self.cancel_drag(sessions) {
            return true;
        }
        if self
            .sketch
            .as_mut()
            .is_some_and(|mode| mode.menu_at.take().is_some())
        {
            return true;
        }
        if self
            .sketch
            .as_ref()
            .is_some_and(|mode| mode.offer().is_some())
        {
            self.sketch_offer(sessions, OfferChoice::Cancel);
            return true;
        }
        self.sketch
            .as_mut()
            .is_some_and(|mode| mode.edit_box.take().is_some())
    }

    /// Esc with no dialog open (section 3's order, the prototype's `escChain`, the nearest
    /// thing first): what takes it before the chain ([`escape_first`](Self::escape_first));
    /// or else sketch mode's tool left, its chain or gesture ended with it (section 9,
    /// *Line*); or else sketch mode's selection cleared, if it has one; or else the reload's
    /// info banner dismissed, if one shows (DESIGN.md, 6c; an amber or red one in its
    /// place has let it go already); or else the feature's selection cleared; or else, with
    /// none of them, a rebuild running stopped quietly
    /// ([`stop_quietly`](Self::stop_quietly)).
    pub fn escape<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if self.escape_first(sessions) {
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
    /// The newest model published, `published`, counted in
    /// [`published_models`](Self::published_models) if it's another than the last seen, though
    /// maybe of the same revision (a fresh check's), so the viewport reads its meshes again.
    fn count_published(&mut self, published: &Response<silico_core::command::Published>) {
        let model = published
            .result
            .as_ref()
            .ok()
            .map(|published| Arc::downgrade(&published.model));
        let same = match (&self.published_model, &model) {
            (Some(seen), Some(now)) => seen.ptr_eq(now),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.published_models = self.published_models.wrapping_add(1);
            self.published_model = model;
        }
    }

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
        // A fresh check publishes its model before it answers, so this read, sent in the poll
        // that took the answer, can be of a model published since that poll counted them:
        // counted now, so the read records the newest.
        self.count_published(&sessions.published(&self.id));
        self.timeline_read_after = self.published_models;
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

    /// A transaction begun if none is open or beginning (decision 1). Whether it began one.
    fn begin_if_none<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        let begins = !self.transaction && !self.beginning;
        if begins {
            self.beginning = true;
            self.record(Sent::Begin);
            self.under.push(Under::Begin {
                sent_at: self.revision,
                running: sessions.begin(&self.id),
            });
        }
        begins
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
    /// state the session is at (the one its step is made from), saying `step`, if it's said
    /// as it's sent (none for what its answer names).
    fn sending(&self, step: Option<Words>) -> Sending {
        Sending {
            against: self.revision,
            state: self.state_of(self.revision),
            step,
        }
    }

    /// An edit's or an add's answer, `response`, to what was `sending`
    /// ([`applied_as`](Self::applied_as)), saying what it was sent saying and naming nothing.
    fn applied<T>(&mut self, sending: &Sending, response: &Response<T>) {
        self.applied_as(sending, response, None, Effect::Same);
    }

    /// An edit's or an add's answer, `response`, to what was `sending`: if it was applied
    /// (taken, and the session's revision moved past the one it was sent against), the
    /// dialogs' groups and the steps past the state it was made from let go, as it dropped
    /// the redo branch they're on (a group ending after that state was undone to reach it);
    /// the state it made kept as a step, saying `words`, what its answer says (a rename's, a
    /// deletion's, an add's), or else what it was sent saying (or else [`Words::edit`]), from
    /// that state; and that state's names, as the state it was made from has them and
    /// `effect` changes them, if they're known there. A refused change, or one that changed
    /// nothing, leaves the redo branch whole, and them with it.
    fn applied_as<T>(
        &mut self,
        sending: &Sending,
        response: &Response<T>,
        words: Option<Words>,
        effect: Effect,
    ) {
        if response.result.is_ok()
            && let Some(made) = response
                .session
                .as_ref()
                .map(|session| session.revision)
                .filter(|revision| *revision != sending.against)
        {
            let from = sending.state;
            self.groups.retain(|group| group.end <= from);
            self.steps.retain(|state, _| *state <= from);
            self.steps.insert(
                made,
                Step {
                    words: words
                        .or_else(|| sending.step.clone())
                        .unwrap_or_else(Words::edit),
                    from,
                },
            );
            self.names.retain(|state, _| *state <= from);
            let names = match effect {
                Effect::Unknown => None,
                Effect::Same => self.names.get(&from).cloned(),
                Effect::Names(feature, name) => self.names.get(&from).cloned().map(|mut names| {
                    names.since.insert(feature, name);
                    names
                }),
            };
            if let Some(names) = names {
                self.names.insert(made, names);
            }
            self.redo_fence = None;
            self.sketch_edited = true;
        }
    }

    /// The features' names the timeline `timeline` lists, kept for the state it's of, shared
    /// with the newest kept if they're the same; and those of states no step was made from or
    /// made, no change under way was sent from, nor the session's now, let go.
    fn took_names(&mut self, timeline: &TimelineAnswer) {
        let Some(features) = timeline.features.as_ref() else {
            return;
        };
        let listed: BTreeMap<FeatureId, String> = features
            .iter()
            .map(|entry| (entry.id.clone(), entry.name.clone()))
            .collect();
        let listed = match self
            .names
            .values()
            .next_back()
            .filter(|names| *names.listed == listed)
        {
            Some(names) => Arc::clone(&names.listed),
            None => Arc::new(listed),
        };
        let state = self.state_of(timeline.revision);
        self.names.insert(
            state,
            Names {
                listed,
                since: BTreeMap::new(),
            },
        );
        let now = self.state_of(self.revision);
        let mut kept: std::collections::BTreeSet<Revision> = self.steps.keys().copied().collect();
        kept.extend(self.steps.values().map(|step| step.from));
        kept.extend(
            self.groups
                .iter()
                .flat_map(|group| [group.start, group.end]),
        );
        // And those a change under way was sent from: the state it makes takes them.
        kept.extend(
            self.edits
                .in_flight
                .as_ref()
                .map(|flying| flying.sending.state),
        );
        kept.extend(self.drag_sending.as_ref().map(|sending| sending.state));
        kept.extend(self.under.iter().filter_map(Under::made_from));
        kept.insert(now);
        self.names.retain(|state, _| kept.contains(state));
    }

    /// What the step `queued` makes says, as it's sent. None means one of two things: it
    /// changes nothing of the document, or its answer names it (a rename, a deletion, an add:
    /// [`applied_as`](Self::applied_as)). A dialog's or sketch mode's own changes say what
    /// they change ("Edit Pad", "Edit Sketch1"), its OK or Finish making them one step of the
    /// dialog's or the sketch's own ([`Queued::Closed`]'s `words`); every other one what it
    /// does to which feature, named by the name it has in the state the step is made from
    /// ([`step_text`](Self::step_text)).
    fn step_words(&self, queued: &Queued) -> Option<Words> {
        match queued {
            Queued::Edit(_, step) => step.clone(),
            Queued::Move { feature, .. } => Some(Words::Of("Move", feature.clone())),
            Queued::Suppress {
                feature,
                suppressed,
            } => Some(Words::Of(
                if *suppressed {
                    "Suppress"
                } else {
                    "Unsuppress"
                },
                feature.clone(),
            )),
            Queued::SketchRemove { sketch, .. }
            | Queued::SketchJoin { sketch, .. }
            | Queued::SketchAdd { sketch, .. }
            | Queued::SketchTrim { sketch, .. }
            | Queued::SketchSplit { sketch, .. }
            | Queued::SketchExtend { sketch, .. }
            | Queued::DimensionValue { sketch, .. }
            | Queued::Offer {
                request:
                    OfferRequest::SetField {
                        feature: sketch, ..
                    },
                ..
            } => Some(Words::Of("Edit", sketch.clone())),
            // As its banner's button names it (Fable's decision of 2026-11-04: by the revision
            // it puts back, never the prototype's "my version").
            Queued::Replace { revision, .. } => Some(Words::Text(format!(
                "Restore revision {}",
                revision.number()
            ))),
            // Named by their answers ([`applied_as`](Self::applied_as)): the feature removed,
            // the feature added.
            Queued::Remove { .. } | Queued::Add { .. } | Queued::AddSketch { .. } => None,
            Queued::Offer {
                request: OfferRequest::Undo { .. },
                ..
            }
            | Queued::Save { .. }
            | Queued::SaveAs { .. }
            | Queued::Revert { .. }
            | Queued::Reload
            | Queued::Step { .. }
            | Queued::Opened { .. }
            | Queued::Load { .. }
            | Queued::Closed { .. }
            | Queued::Restore { .. } => None,
        }
    }

    /// What sketch mode's step says, `verb` and its sketch: by the sketch's ID once its add
    /// has answered; till then (Create Sketch's), by the name its add gives it, or "Sketch",
    /// as the sketch bar says it.
    fn sketch_words(&self, verb: &'static str) -> Words {
        let mode = self.sketch.as_ref();
        match mode.and_then(|mode| mode.sketch.clone()) {
            Some(sketch) => Words::Of(verb, sketch),
            None => {
                let name = mode
                    .and_then(|mode| mode.created.as_deref())
                    .map_or_else(|| "Sketch".to_owned(), step_name);
                Words::Text(format!("{verb} {name}"))
            }
        }
    }

    /// What the one step `dialog`'s edits make once it's OK'd says: "Add" and its feature
    /// for a new feature's, "Edit" and it for an existing one's (the prototype's `okDialog`,
    /// `'Edit ' + D.name`; Create Sketch's Finish, `'Add ' + name`); a new one's whose add
    /// hasn't answered, by its kind's title ([`Kind::title`]), the name its add gives it.
    fn dialog_step(&self, dialog: &Dialog) -> Words {
        dialog_words(dialog, if dialog.made { "Add" } else { "Edit" })
    }

    /// What the step an edit asked now makes says ([`edit`](Self::edit)), as it's asked:
    /// none for a rename, wherever it's made, which its answer names, saying the name it
    /// replaced (the prototype's `'Rename ' + f.name`: [`renaming`]); else the open dialog's
    /// ("Edit Pad"), if one is open; else sketch mode's, if it's open; else "Edit" and the
    /// feature.
    fn edit_words(&self, edit: &Edit) -> Option<Words> {
        if edit.field == "name" {
            return None;
        }
        Some(if let Some(dialog) = &self.dialog {
            dialog_words(dialog, "Edit")
        } else if self.sketch.is_some() {
            self.sketch_words("Edit")
        } else {
            Words::Of("Edit", edit.feature.clone())
        })
    }

    /// An add's answer, `response`, to what was `sending`: applied, saying "Add" and the
    /// feature's name as it answers it, and naming the state it made with it ([`made_by`]).
    fn took_added(&mut self, sending: &Sending, response: &Response<Added>) {
        match &response.result {
            Ok(added) => {
                let (words, effect) = made_by(&added.feature);
                self.applied_as(sending, response, Some(words), effect);
            }
            Err(_) => self.applied(sending, response),
        }
    }

    /// The tip of the document as the tab's timeline last listed it ([`tip_of`]): whether a
    /// sweep that cuts has a body to cut, for CREATE's tools. A sweep's add takes its body
    /// from the document as it is when it's sent ([`base_of`]).
    fn tip(&self) -> Option<FeatureId> {
        tip_of(self.timeline.as_ref()?.features.as_ref()?)
    }

    /// Whether the timeline lists a sketch of ID `id`.
    fn is_sketch(&self, id: &FeatureId) -> bool {
        self.browser.sketches().iter().any(|row| &row.id == id)
    }

    /// Whether `what`'s row shows (its eye open): its override if it has one, else its
    /// default ([`shown_in`]).
    pub fn shown(&self, what: &Hideable) -> bool {
        shown_in(&self.overrides, &self.browser, what)
    }

    /// `what` hidden if it shows, shown if it's hidden: its eye clicked, or H ("the eye and
    /// H set an override", decision 29). Back to its default, its override is dropped, so
    /// two clicks leave none; away from it, the override is set, with a sketch's consumers
    /// now.
    pub fn toggle_shown(&mut self, what: Hideable) {
        let shown = !self.shown(&what);
        if shown == shown_by_default(&self.browser, &what) {
            self.overrides.remove(&what);
        } else {
            let under = sketch_row(&self.browser, &what)
                .map(|row| row.consumers.clone())
                .unwrap_or_default();
            self.overrides.insert(what, Override { shown, under });
        }
    }

    /// Why the sketch of `row` is drawn nowhere, whatever its eye, for the eye's tooltip:
    /// suppressed, or its read placing it on no plane (a face gone,
    /// found by an unstable name, not planar, or its feature unbuilt: `plane_problem`'s
    /// message), since nothing guesses a placement. None for one placed, or not read yet.
    pub fn not_drawn(&self, row: &Row) -> Option<String> {
        if row.suppressed {
            return Some("Not drawn: suppressed".to_owned());
        }
        let held = self.sketch_reads.held(&row.id)?;
        held.read
            .plane
            .as_ref()
            .err()
            .map(|problem| format!("Not drawn: {}", problem.message()))
    }

    /// The features no sketch counts among its consumers yet ("a feature the open dialog
    /// is making counts from its OK", decision 29): the one the open dialog is making, till
    /// its OK; and each cancelled one's kept in `unmade`.
    fn making(&self) -> Vec<&FeatureId> {
        self.dialog
            .as_ref()
            .filter(|dialog| dialog.made)
            .and_then(|dialog| dialog.feature.as_ref())
            .into_iter()
            .chain(self.unmade.iter().map(|unmade| &unmade.feature))
            .collect()
    }

    /// What `lands` names landed where an undo's or rollback's answer, of `session`, says the
    /// session came to: its revision then, never one guessed ahead (another client's edit
    /// may come between), or the tab's own for an answer naming no session.
    fn landed(&mut self, lands: Lands, session: Option<&SessionInfo>) {
        let at = session.map_or(self.revision, |session| session.revision);
        self.land(lands, at);
    }

    /// What `lands` names landed at `at`, the revision its undo or rollback answered at (or
    /// the session's, if it sent none): a dialog's Cancel, its unmade features and its Edit
    /// Feature's freeze; a revert, every one not landed yet (it undoes them all). Each is
    /// then kept from answers older than `at` only.
    fn land(&mut self, lands: Lands, at: Revision) {
        let of = |serial: u64| match lands {
            Lands::Nothing => false,
            Lands::Dialog(landing) => serial == landing,
            Lands::Everything => true,
        };
        for unmade in &mut self.unmade {
            if unmade.landing == Landing::Unsent && of(unmade.serial) {
                unmade.landing = Landing::At(at);
            }
        }
        if let Some((serial, landing @ Landing::Unsent)) = &mut self.edit_undone
            && of(*serial)
        {
            *landing = Landing::At(at);
        }
    }

    /// The browser's rows worked out again from the tab's timeline, the features no sketch
    /// counts yet ([`making`](Self::making)) left out of each sketch's consumers, and each
    /// sketch's override dropped if a feature newly counts among them, or else its
    /// consumers taken as its own ("kept until the tab closes or a feature is newly built
    /// from the sketch", decision 29). A sketch the timeline doesn't list keeps its
    /// override, as it was left. While an Edit Feature dialog is open, and for an answer
    /// older than the undo of one cancelled, no override changes: "Edit Feature … drop
    /// nothing", a move cancelled included; one OK'd counts at its OK, its new sketch's
    /// override dropped and its old one's kept.
    fn rebrowse(&mut self) {
        let Some(timeline) = self.timeline.as_ref() else {
            return;
        };
        let at = timeline.revision;
        self.unmade.retain(|unmade| unmade.landing.after(at));
        let edit_undone = match self.edit_undone {
            Some((_, landing)) if !landing.after(at) => {
                self.edit_undone = None;
                false
            }
            Some(_) => true,
            None => false,
        };
        let editing = self.dialog.as_ref().is_some_and(|dialog| !dialog.made);
        let rows = browser(timeline, &self.making());
        self.browser = rows;
        if editing || edit_undone {
            return;
        }
        for row in self.browser.sketches() {
            let what = Hideable::Feature(row.id.clone());
            let gained = self
                .overrides
                .get(&what)
                .map(|kept| !row.consumers.is_subset(&kept.under));
            match gained {
                Some(true) => {
                    self.overrides.remove(&what);
                }
                Some(false) => {
                    if let Some(kept) = self.overrides.get_mut(&what) {
                        kept.under = row.consumers.clone();
                    }
                }
                None => {}
            }
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
            .chain(self.browser.sketches())
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
        self.can_open(kind) && (!kind.combines() || self.browser.bodies().len() >= 2)
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
        (kind.combines() && self.browser.bodies().len() < 2).then_some(NEEDS_TWO_BODIES)
    }

    /// A new feature's dialog opened (section 9): its inputs at their defaults, or those its
    /// kind was last OK'd with, if it remembers them (`remembered`); a sweep's sketch the one
    /// selected if it's one, and its body its sketch's face's, or the tip ([`base_of`]). Its
    /// feature is added as soon as its inputs are valid.
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
                && self.browser.bodies().iter().any(|row| &row.id == id)
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
    /// the last one left let go ([`new_dialog`](Self::new_dialog)). Its number.
    fn opening(&mut self) -> u64 {
        let serial = self.new_dialog();
        self.edits.queue.push_back(Queued::Opened { serial });
        serial
    }

    /// A dialog or sketch mode opening at `at`, the one writer of its opening: where its
    /// Cancel undoes back to and its OK's or Finish's one step starts; whose the transaction
    /// since then is (`owner`); and nothing of the tab's applied since. Sketch mode's own
    /// opening (what its undo stops at) moves only if `serial` is sketch mode's number.
    fn opens_at(&mut self, at: Revision, owner: Owner, serial: Option<u64>) {
        self.dialog_base = Some(at);
        self.dialog_began = owner == Owner::Opened;
        self.sketch_edited = false;
        if let Some(mode) = self
            .sketch
            .as_mut()
            .filter(|mode| Some(mode.serial) == serial)
        {
            mode.opened_at = Some(at);
        }
    }

    /// Whether a dialog's or sketch mode's close is queued, not yet taken.
    fn close_queued(&self) -> bool {
        self.edits
            .queue
            .iter()
            .any(|queued| matches!(queued, Queued::Closed { .. }))
    }

    /// A dialog numbered, the next, and what the last one left (a refusal said, its refused
    /// fields, the selection its box took) let go. Its number.
    fn new_dialog(&mut self) -> u64 {
        self.hush();
        self.refused.clear();
        self.before_dialog = None;
        self.dialogs += 1;
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
        self.queue_read(serial, kind, feature);
        self.advance(sessions);
    }

    /// The feature `feature`'s text read for the dialog numbered `serial`, of `kind`, queued:
    /// sent by the next [`advance`](Self::advance).
    fn queue_read(&mut self, serial: u64, kind: Kind, feature: FeatureId) {
        self.loading = Some(serial);
        self.edits.queue.push_back(Queued::Load {
            serial,
            kind,
            feature,
        });
    }

    /// The session reloaded its file, to the revision `to`, with a dialog or sketch mode
    /// open. A session reloads only with no transaction open, so each opens at `to` with none
    /// open there: whatever is sent after it is its own, a transaction its own edits'
    /// `begin` opened as it reloaded the file included. An existing feature's dialog is read
    /// again, its values the file's; a new feature's not yet added stays open; one already
    /// added can't have been reloaded under (its add opened a transaction), but if it was,
    /// it's closed, said.
    ///
    /// The read is queued, never sent here: a reload is seen as a poll reads `status` or takes
    /// its answers, and the advance after them sends it, behind what's still under way, or
    /// not at all once a save or a revert closing the tab has let go of what was queued.
    fn reloaded_under_dialog(&mut self, to: Revision) {
        // Sketch mode on a sketch with nothing of its own sent (a session reloads only with
        // no transaction open, and Create Sketch's add opens one): it opens at the reloaded
        // revision, its next read the reloaded sketch's.
        if let Some(mode) = self.sketch.as_mut() {
            // What its tool added is the file's no longer: forgotten, with its gesture.
            mode.tools.forget_pending();
            let serial = mode.serial;
            self.opens_at(to, Owner::Opened, Some(serial));
        } else if self.dialog_base.is_some() && self.close_queued() {
            // A dialog or sketch mode gone, its close still queued (OK, Cancel, or Finish
            // after its edit was sent, that edit's begin reloading the file; the close waits
            // for that answer): the close ends what came after the reload, a transaction that
            // begin opened included.
            self.opens_at(to, Owner::Opened, None);
        }
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        match (dialog.made, dialog.feature.clone()) {
            (false, Some(feature)) => {
                // Opened at `to` now, not as its read is queued: an edit queued before the
                // read is sent after the reload, its own.
                let serial = self.new_dialog();
                self.opens_at(to, Owner::Opened, None);
                self.queue_read(serial, dialog.kind, feature);
            }
            (true, None) => {
                self.opens_at(to, Owner::Opened, None);
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
                let words = Words::Then(broken.feature.clone(), crate::repair::USED_STEP);
                self.edit_with_words(sessions, edit, Some(words));
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
                let words = Words::Then(broken.feature.clone(), crate::repair::REMOVED_STEP);
                self.edit_with_words(sessions, edit, Some(words));
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
        // A new sweep's base worked out before its add is sent, as the add will work it out:
        // a body no feature may take sends nothing, and the dialog says why up front. A
        // dialog with no feature yet is a new one's (Edit Feature's has its feature from the
        // first), so being made needs no asking.
        if dialog.kind.sweeps() && dialog.feature.is_none() {
            let entries = self
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.features.as_deref());
            let at = self.timeline.as_ref().map(|timeline| timeline.revision);
            if base_refused_before_add(dialog, entries, at) {
                return false;
            }
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
        // A refusal of a sketch the box no longer holds (the one the document has picked
        // back) goes: else, the box's sketch being no change, nothing would clear it.
        if dialog
            .base_refused
            .as_ref()
            .is_some_and(|refused| Some(&refused.sketch) != dialog.sketch())
        {
            dialog.base_refused = None;
        }
        let sketch_field = crate::dialog::Part::Sketch.field();
        let sketch_sent = dialog.sent(sketch_field).cloned();
        let changes = dialog.changes(waiting);
        // A new sweep's sketch picked after its add: its base follows, to where an add from
        // that sketch would build (#443's review, H1), worked out from the newest timeline as
        // the add's was. A sweep's own dialog (Edit Feature) keeps the base it has.
        let picked = changes.iter().any(|(field, _)| *field == sketch_field);
        let moved = match (
            dialog.made && dialog.kind.sweeps() && picked,
            dialog.sketch(),
        ) {
            (true, Some(sketch)) => self
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.features.as_deref())
                .map(|entries| (sketch.clone(), rebased(entries, &feature, sketch))),
            _ => None,
        };
        let mut changes = changes;
        let moved_to = match moved {
            // A body no feature may take: the sketch isn't sent (nothing builds on another
            // body for it), OK is off, and the dialog says why.
            Some((sketch, Err(why))) => {
                dialog.base_refused = Some(crate::dialog::BaseRefused {
                    sketch,
                    why,
                    read: None,
                });
                // Held back: taken as unsent, so it's offered again each frame, and sent
                // once the tab's timeline lets the body be taken (another client's edit
                // lifting the refusal).
                changes.retain(|(field, _)| *field != sketch_field);
                dialog.unsend(sketch_field, sketch_sent);
                None
            }
            Some((_, Ok(base))) => {
                dialog.base_refused = None;
                base.filter(|base| dialog.base.as_ref() != Some(base))
            }
            None => None,
        };
        if let Some(base) = &moved_to {
            dialog.base = Some(base.clone());
        }
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
        // Its base sent after its sketch, in the dialog's transaction, so OK keeps both and
        // Cancel undoes both.
        if let Some(value) = moved_to.and_then(|base| {
            silico_core::document::json::read(quoted(base.as_str()).as_bytes()).ok()
        }) {
            self.edit(
                sessions,
                Edit {
                    feature,
                    field: crate::dialog::BASE.to_owned(),
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
        let serial = closed.as_ref().map(|closed| closed.serial);
        // What the one step its OK makes of its edits says: its feature, named from the state
        // it opened at once the step is made ([`dialog_step`](Self::dialog_step)).
        let words = closed
            .as_ref()
            .filter(|_| ok)
            .map(|closed| self.dialog_step(closed));
        let before = self.before_dialog.take();
        // What it leaves counted, once its opening has been taken (one still queued has made
        // nothing).
        let opened = serial.is_some_and(|serial| {
            !self
                .edits
                .queue
                .iter()
                .any(|queued| matches!(queued, Queued::Opened { serial: s } if *s == serial))
        });
        if let Some(base) = self.dialog_base.filter(|_| opened) {
            self.count_closed(ok, base);
        }
        if ok {
            self.oked = closed;
            // Its feature, made, counts from now: its sketch gains it. An edit counts now
            // too, its new sketch's override dropped.
            self.rebrowse();
        } else {
            self.dropped(closed.as_ref());
            if let Some(items) = before {
                self.selection.restore(items);
            }
        }
        self.edits.queue.push_back(Queued::Closed {
            ok,
            sketch: false,
            dialog: serial,
            words,
        });
        self.advance(sessions);
    }

    /// `dialog`, closed without OK or dropped by a revert, its edits undone or rolled back:
    /// a new feature's, if its add has answered, counted as no sketch's consumer in an
    /// answer older than where that lands ([`land`](Self::land)), so a read answered before
    /// the undo gains its sketch nothing; an Edit Feature's, no override changed till then.
    fn dropped(&mut self, dialog: Option<&Dialog>) {
        let Some(dialog) = dialog else {
            return;
        };
        if !dialog.made {
            self.edit_undone = Some((dialog.serial, Landing::Unsent));
        } else if let Some(feature) = dialog.feature.clone() {
            self.add_outlived(dialog.serial, feature);
        }
    }

    /// The add of the dialog numbered `serial` answered, making `feature`: the dialog's own,
    /// if it's still open, and selected, so its mark's text is read and the property panel
    /// shows it (but not over the elements a box of names takes from the selection, whose
    /// mark's text is read for the dialog's feature, `read_check`); or, its dialog closed
    /// before it answered, unmade ([`add_outlived`](Self::add_outlived)).
    fn add_answered(&mut self, serial: u64, feature: FeatureId) {
        match self
            .dialog
            .as_mut()
            .filter(|dialog| dialog.serial == serial)
        {
            Some(dialog) => {
                dialog.added(feature.clone());
                if dialog.takes().is_none() {
                    self.selection.select_feature(feature);
                }
            }
            None => self.add_outlived(serial, feature),
        }
    }

    /// `feature`, added by the dialog numbered `serial`, which is closed without OK or
    /// dropped (before or after its add answered): unmade, till where that dialog's Cancel,
    /// or a revert, lands.
    fn add_outlived(&mut self, serial: u64, feature: FeatureId) {
        if !self.unmade.iter().any(|unmade| unmade.feature == feature) {
            self.unmade.push(Unmade {
                feature,
                serial,
                landing: Landing::Unsent,
            });
        }
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
            && !self.close_queued()
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
        let name = format!("Sketch{}", self.browser.sketches().len() + 1);
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
        let mut mode = SketchMode::new(serial, None);
        mode.created = Some(name);
        self.sketch = Some(mode);
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
        // The one step its Finish makes of its edits: "Add" and the sketch's name for Create
        // Sketch's (the prototype's `'Add ' + name`), "Edit" and it for Edit Sketch's
        // (`'Edit Bracket profile'`).
        let words = self.sketch.as_ref().filter(|_| ok).map(|mode| {
            self.sketch_words(if mode.created.is_some() {
                "Add"
            } else {
                "Edit"
            })
        });
        if let Some(base) = self.sketch.as_ref().and_then(|mode| mode.opened_at) {
            self.count_closed(ok, base);
        }
        // Its close waits for a drag's end (the queue waits while one is under way).
        self.leave_drag();
        self.sketch = None;
        self.drive_drag(sessions);
        self.edits.queue.push_back(Queued::Closed {
            ok,
            sketch: true,
            dialog: None,
            words,
        });
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
            // Its end's edit, a step of sketch mode's own.
            self.drag_sending = Some(self.sending(Some(Words::Of("Edit", drag.sketch.clone()))));
            drag.begun(sessions.drag_begin(&self.id, drag.sketch.clone(), drag.grab.clone()));
        }
        for news in drag.poll(sessions, &self.id) {
            match news {
                News::EndSent => self.record(Sent::SketchEdit("drag_end")),
                News::Refused(problems) => self.message = Some(said(&problems)),
                News::Ended(response) => {
                    self.took_session(&response);
                    if let Some(sending) = self.drag_sending.take() {
                        self.applied(&sending, &response);
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
        let is_body = self.browser.bodies().iter().any(|row| &row.id == id);
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

    /// A new sweep's base read (`Under::Base`), answered with `read`: the add queued next,
    /// before anything queued after it, with the base worked out from the sketch it sends
    /// ([`base_of`]) at the revision read; read again if the read was refused (it names no
    /// model when none is published, so it's refused only as every read is). A body no
    /// feature may take sends nothing: the dialog of `serial`, if open, forgets the add
    /// ([`Dialog::forget_add`]) and says why, kept while the tab's timeline is older than
    /// this read ([`base_refused_before_add`]).
    fn base_read(
        &mut self,
        serial: u64,
        kind: Kind,
        members: Vec<String>,
        read: Result<TimelineAnswer, Problems>,
    ) {
        let body = match read {
            Ok(timeline) => {
                let sketch = crate::dialog::sketch_of(&members);
                let base = match timeline.features.as_deref() {
                    Some(entries) => base_of(entries, sketch.as_ref()),
                    None => Ok(None),
                };
                match base {
                    Ok(base) => Body::Read(base, timeline.revision),
                    Err(why) => {
                        if let Some(dialog) = self
                            .dialog
                            .as_mut()
                            .filter(|dialog| dialog.serial == serial)
                        {
                            dialog.forget_add();
                            dialog.base_refused = sketch.map(|sketch| crate::dialog::BaseRefused {
                                sketch,
                                why,
                                read: Some(timeline.revision),
                            });
                        }
                        return;
                    }
                }
            }
            Err(_) => Body::Unread,
        };
        self.edits.queue.push_front(Queued::Add {
            serial,
            kind,
            members,
            body,
        });
    }

    /// The add of the dialog of `serial` refused, `problems` saying why, which is said. Made
    /// against a revision the session has moved on from (another client's edit came first):
    /// let go ([`Dialog::forget_add`]), so the next pass sends it again, at the revision now.
    /// Any other refusal leaves it unsent till an input changes; either way OK stays off
    /// while no feature is made ([`ok_for`](Self::ok_for)).
    fn add_refused(&mut self, serial: u64, problems: &Problems) {
        let stale = problems.errors().iter().any(|error| {
            error.code() == silico_core::ErrorCode::Conflict
                && error.parameter() == Some("revision")
        });
        if let Some(dialog) = self
            .dialog
            .as_mut()
            .filter(|dialog| dialog.serial == serial)
        {
            if stale {
                dialog.forget_add();
            } else {
                dialog.adding = false;
            }
        }
        self.message = Some(said(problems));
    }

    /// Whether the dialog's inputs let it be OK'd ([`ok_for`](Self::ok_for)): valid, no edit
    /// of one refused, a new feature's add answered with its feature (what's typed meanwhile
    /// is sent after it). While the tab closes, [`close_dialog`](Self::close_dialog) takes
    /// neither OK nor Cancel.
    pub fn can_ok(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| self.ok_for(dialog))
    }

    /// Whether `dialog`, the tab's open one (taken out of the tab to be drawn, or not), can
    /// be OK'd: valid, a new feature's add answered with its feature (not refused, nor still
    /// to answer), no box holding what isn't sent, no edit of an input refused, and a sweep's
    /// base neither refused nor waiting (its sketch and its base are one change). The one
    /// rule the button and Enter take.
    pub(crate) fn ok_for(&self, dialog: &Dialog) -> bool {
        let refused = |field: &str| {
            dialog
                .feature
                .as_ref()
                .is_some_and(|feature| self.refused.contains(&(feature.clone(), field.to_owned())))
        };
        // A dialog with no feature has none to OK: a new one's add unanswered or refused
        // (Edit Feature's has its feature from the first).
        dialog.feature.is_some()
            && !dialog.adding
            && !dialog.holding()
            && dialog.valid()
            && !dialog
                .inputs
                .iter()
                .any(|input| refused(input.part.field()))
            && dialog.base_refused.is_none()
            && !self.base_held_by(dialog)
    }

    /// Whether the open dialog, a sweep's, waits on its own edit of its `base` (queued or
    /// in flight), or had it refused: OK is off meanwhile, so its sketch is never kept
    /// without the base it was picked with. Never for another dialog (a boolean's base box
    /// is an input of its own, held by its box).
    pub fn base_held(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| self.base_held_by(dialog))
    }

    /// [`base_held`](Self::base_held) of `dialog`, the tab's open one.
    fn base_held_by(&self, dialog: &Dialog) -> bool {
        dialog.kind.sweeps()
            && dialog.feature.as_ref().is_some_and(|feature| {
                self.edits.pending_field(feature, crate::dialog::BASE)
                    || self
                        .refused
                        .contains(&(feature.clone(), crate::dialog::BASE.to_owned()))
            })
    }

    /// Whether the last edit of `field` of the dialog's feature was refused.
    pub fn field_refused(&self, field: &str) -> bool {
        self.dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref())
            .is_some_and(|feature| self.refused.contains(&(feature.clone(), field.to_owned())))
    }

    /// What the dialog says beneath its inputs: why a new sweep can't be built on its
    /// sketch's body, if it can't (`Dialog::base_refused`); its feature's error (or warning),
    /// as the preview's check gives it (its message, cause, elements, and suggestion); then
    /// the tab's last refusal.
    pub fn dialog_said(&self) -> Vec<Said> {
        let feature = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref());
        let refused = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.base_refused.as_ref())
            .map(|refused| Said {
                message: refused.why.clone(),
                ..Said::default()
            });
        refused
            .into_iter()
            .chain(
                feature
                    .and_then(|feature| self.said.get(feature.as_str()))
                    .cloned(),
            )
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

/// What a document's features, as `timeline` lists them, consume, each by the feature that
/// consumes it: every feature but a sketch that a feature other than a sketch names as an
/// input, built or not, suppressed or not, as the gate counts consumers
/// (`Operation::inputs`: a feature's base and tool). The gate lets a solid have one consumer
/// at most; were there more, the first in the timeline's order is kept. A sketch consumes
/// nothing, so a sketch on a face leaves the face's feature standing; and a sketch isn't
/// consumed, so the sketch a sweep names, among the sweep's inputs, isn't here, as the gate
/// lets any number of features use one. The one rule for the tip ([`tip_of`]), a sweep's
/// base ([`base_of`]), and the feature menus' rows (`crate::feature_menu`).
pub(crate) fn consumed(entries: &[Entry]) -> BTreeMap<&FeatureId, &FeatureId> {
    let sketches: std::collections::BTreeSet<&FeatureId> = entries
        .iter()
        .filter(|entry| entry.kind == "sketch")
        .map(|entry| &entry.id)
        .collect();
    let mut consumed = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry.kind != "sketch") {
        for input in entry
            .inputs
            .iter()
            .filter(|input| !sketches.contains(input))
        {
            consumed.entry(input).or_insert(&entry.id);
        }
    }
    consumed
}

/// Which of a document's features, as `timeline` lists them, in order, stand for a solid: a
/// feature that makes one (it isn't a sketch) and isn't suppressed, or one that's suppressed
/// whose base stands for one (its chain passes it through, as `document::suppression`'s
/// `standing` has it).
fn solids(entries: &[Entry]) -> BTreeMap<&FeatureId, bool> {
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
    stands
}

/// The tip of a document, as FreeCAD's PartDesign takes its body's, from its features as
/// `timeline` lists them, in order: the last that stands for a solid ([`solids`]) and that
/// no feature consumes ([`consumed`]).
fn tip_of(entries: &[Entry]) -> Option<FeatureId> {
    let consumed = consumed(entries);
    let stands = solids(entries);
    entries
        .iter()
        .rev()
        .find(|entry| {
            stands.get(&entry.id).copied().unwrap_or(false) && !consumed.contains_key(&entry.id)
        })
        .map(|entry| entry.id.clone())
}

/// The body a new sweep from the sketch `sketch` builds on, from a document's features as
/// `timeline` lists them, in order.
///
/// From a sketch on a face, it's the face's body: the face's feature (the sketch's input,
/// `[face's feature]`), followed forward through the feature that consumes each one
/// ([`consumed`]) to the end of its chain, if that stands for a solid ([`solids`]). So in a
/// part of two bodies, a pocket sketched on the first body's face cuts that body, not the
/// last one built. Behaviour follows SolidWorks, where a feature from a sketch on a face
/// merges with that face's body (an extrude's *Merge result*); FreeCAD's PartDesign makes
/// the same choice, building a Pad or Pocket in the sketch's own Body, after its Tip
/// (`prepareProfileBased`, src/Mod/PartDesign/Gui/Command.cpp), and offering a sketch of
/// another Body only as a copy or a cross-reference into the active one
/// (`TaskFeaturePick::otherBody`, the same file).
///
/// A suppressed feature on the way consumes nothing and passes only its base through
/// (`document::suppression`, and the core's `bodies_of`): the walk goes on through it only
/// from its base. From its tool, the walk ends at the body, which the core lists as a body
/// of its own, but which no feature may take as its base: the gate counts the suppressed
/// boolean as its consumer, suppressed or not (`Operation::inputs`), and refuses a second.
/// So that's no base but a refusal, saying why in the app's words: no feature is built on
/// another body in its place (#443's review, M2).
///
/// A sketch in error (its face not found, `reference_broken`; a conflict; blocked
/// upstream; drift) is on its face's body all the same: the document records its face's
/// feature as the intent, and the sketch, once fixed, is there.
///
/// Otherwise it's the tip ([`tip_of`]), as it is with one body: with no sketch, from a
/// sketch on an origin plane (no input), from a sketch whose face's feature the timeline
/// doesn't list (removed), or where the chain's end stands for no solid.
fn base_of(entries: &[Entry], sketch: Option<&FeatureId>) -> Result<Option<FeatureId>, String> {
    match sketch.and_then(|sketch| face_body(entries, sketch)) {
        Some(found) => found.map(Some),
        None => Ok(tip_of(entries)),
    }
}

/// Whether a new sweep's dialog, its add not sent, may not send it: its base worked out
/// against `entries`, the tab's timeline, of the revision `at`, as its add will work it out
/// ([`base_of`]), a body no feature may take refused, `dialog.base_refused` saying why. A
/// refusal of the add's own read of the session's document (`Under::Base`,
/// [`BaseRefused::read`]) for the same sketch stands while the timeline is older than that
/// read: else it could clear the refusal, and turn OK on with nothing added. Once the
/// timeline is of that revision or newer, or the sketch changed, the check decides.
///
/// [`BaseRefused::read`]: crate::dialog::BaseRefused::read
fn base_refused_before_add(
    dialog: &mut Dialog,
    entries: Option<&[Entry]>,
    at: Option<Revision>,
) -> bool {
    let Some(sketch) = dialog.sketch().cloned() else {
        dialog.base_refused = None;
        return false;
    };
    let read_stands = dialog.base_refused.as_ref().is_some_and(|refused| {
        refused.sketch == sketch
            && refused
                .read
                .is_some_and(|read| at.is_none_or(|at| at < read))
    });
    if read_stands {
        return true;
    }
    match entries.and_then(|entries| base_of(entries, Some(&sketch)).err()) {
        Some(why) => {
            dialog.base_refused = Some(crate::dialog::BaseRefused {
                sketch,
                why,
                read: None,
            });
            true
        }
        None => {
            dialog.base_refused = None;
            false
        }
    }
}

/// [`base_of`] for the open dialog's new sweep `own` from `sketch`, picked into it after its
/// add: of the timeline less the sweep itself, as its add found the document (a new
/// feature's add goes at the end, so nothing else names it), so it neither consumes the body
/// it was added on nor stands as the tip.
fn rebased(
    entries: &[Entry],
    own: &FeatureId,
    sketch: &FeatureId,
) -> Result<Option<FeatureId>, String> {
    let others: Vec<Entry> = entries
        .iter()
        .filter(|entry| &entry.id != own)
        .cloned()
        .collect();
    base_of(&others, Some(sketch))
}

/// The body of the face the sketch `sketch` is on, as [`base_of`] finds it, or why no
/// feature may build on it; none if it's on no face, or its face's body can't be found.
fn face_body(entries: &[Entry], sketch: &FeatureId) -> Option<Result<FeatureId, String>> {
    let sketch = entries
        .iter()
        .find(|entry| &entry.id == sketch && entry.kind == "sketch")?;
    let face = sketch.inputs.first()?;
    let mut end = &entries.iter().find(|entry| &entry.id == face)?.id;
    let consumed = consumed(entries);
    let entry_of = |id: &FeatureId| entries.iter().find(|entry| &entry.id == id);
    // A consumer comes after what it consumes, so no chain is longer than the timeline.
    for _ in 0..entries.len() {
        let Some(next) = consumed.get(end) else {
            break;
        };
        // A suppressed consumer passes only its base through: its first input (a boolean's
        // base before its tool, a sweep's before its sketch, which isn't consumed).
        if let Some(consumer) = entry_of(next)
            && consumer.suppressed
            && consumer.inputs.first() != Some(end)
        {
            let name = |id: &FeatureId| {
                entry_of(id).map_or_else(|| id.to_string(), |entry| entry.name.clone())
            };
            let (body, boolean) = (name(end), name(next));
            return Some(Err(format!(
                "{body} is the tool of {boolean}, which consumes it while suppressed: unsuppress or remove {boolean} to build on it"
            )));
        }
        end = next;
    }
    solids(entries)
        .get(end)
        .copied()
        .unwrap_or(false)
        .then(|| Ok(end.clone()))
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
/// first in error, else the first with a warning); and its sketches, each marked by its own,
/// with its consumers (decision 29): each feature that lists it among its `inputs`, but those
/// of `making`, the feature the open dialog is making, which counts from its OK, and any
/// cancelled one not yet undone in what's read. Each feature found by its ID once, through a
/// map.
pub fn browser(timeline: &TimelineAnswer, making: &[&FeatureId]) -> Browser {
    let entries = timeline.features.as_deref().unwrap_or_default();
    let place: BTreeMap<&FeatureId, usize> = entries
        .iter()
        .enumerate()
        .map(|(at, entry)| (&entry.id, at))
        .collect();
    let mut consumers: BTreeMap<&FeatureId, std::collections::BTreeSet<FeatureId>> =
        BTreeMap::new();
    for entry in entries.iter().filter(|entry| !making.contains(&&entry.id)) {
        for input in &entry.inputs {
            consumers.entry(input).or_default().insert(entry.id.clone());
        }
    }
    let row = |entry: &Entry, worst: Option<usize>| Row {
        id: entry.id.clone(),
        name: entry.name.clone(),
        health: worst.map_or(Health::Healthy, |at| entries[at].health),
        marked_by: worst.map(|at| entries[at].id.clone()),
        consumers: std::collections::BTreeSet::new(),
        suppressed: entry.suppressed,
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
            Row {
                consumers: consumers.remove(&entry.id).unwrap_or_default(),
                ..row(entry, marked.then_some(at))
            }
        })
        .collect();
    Browser::new(bodies, sketches)
}

/// `what`'s sketch row in `browser`, if it's a sketch's and the browser lists it.
fn sketch_row<'a>(browser: &'a Browser, what: &Hideable) -> Option<&'a Row> {
    match what {
        Hideable::Feature(id) => browser.sketches().iter().find(|row| &row.id == id),
        Hideable::Origin(_) => None,
    }
}

/// Whether `what`'s row shows before any click (decision 29: "everything but the sketches
/// a feature is built from"): a body and an origin row, yes; a sketch, while nothing is
/// built from it.
fn shown_by_default(browser: &Browser, what: &Hideable) -> bool {
    match what {
        Hideable::Feature(id) => browser.consumed.get(id).is_none_or(|built| !built),
        Hideable::Origin(_) => true,
    }
}

/// Whether `what`'s row shows: its override in `overrides` if it has one, else its default
/// by `browser`. The one rule [`Tab::shown`] and the sketch reads' order
/// ([`SketchReads::poll`]) both use.
fn shown_in(overrides: &BTreeMap<Hideable, Override>, browser: &Browser, what: &Hideable) -> bool {
    overrides
        .get(what)
        .map_or_else(|| shown_by_default(browser, what), |kept| kept.shown)
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
    fn each_sketch_row_lists_the_features_built_from_it_less_the_one_a_dialog_is_making() {
        // Decision 29's consumers: s1; p1, a pad of s1, suppressed; c1, a
        // pocket of p1 from s1, failed; s2, a sketch on p1's face; s3, built from by nothing.
        // s1's consumers are p1 and c1, suppressed or failed alike; s2 naming p1 makes p1 no
        // sketch's consumer, and s2 built from by nothing; s3 none. The open dialog making
        // c1: s1's are p1 alone. A body's row lists none.
        let id = |text: &str| FeatureId::new(text).unwrap();
        let mut failed = feature("c0000001", "pocket", false, &["p0000001", "s0000001"]);
        failed.health = Health::Error;
        let read = TimelineAnswer {
            bodies: vec![id("p0000001")],
            ..timeline(
                0,
                None,
                vec![
                    feature("s0000001", "sketch", false, &[]),
                    feature("p0000001", "pad", true, &["s0000001"]),
                    failed,
                    feature("s0000002", "sketch", false, &["p0000001"]),
                    feature("s0000003", "sketch", false, &[]),
                ],
            )
        };
        let consumers = |browser: &Browser| -> Vec<(String, Vec<String>)> {
            browser
                .sketches
                .iter()
                .map(|row| {
                    let of = row.consumers.iter().map(ToString::to_string).collect();
                    (row.id.to_string(), of)
                })
                .collect()
        };
        let row = |sketch: &str, of: &[&str]| {
            let of = of.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
            (sketch.to_owned(), of)
        };
        let all = browser(&read, &[]);
        assert_eq!(
            consumers(&all),
            [
                row("s0000001", &["c0000001", "p0000001"]),
                row("s0000002", &[]),
                row("s0000003", &[]),
            ]
        );
        assert_eq!(all.bodies.len(), 1);
        assert!(all.bodies[0].consumers.is_empty());
        assert_eq!(
            consumers(&browser(&read, &[&id("c0000001")])),
            [
                row("s0000001", &["p0000001"]),
                row("s0000002", &[]),
                row("s0000003", &[]),
            ]
        );
    }

    #[test]
    fn a_sketchs_override_falls_to_a_feature_newly_built_from_it_and_outlasts_a_loss() {
        // Decision 29's overrides: Profile (s1), padded by p1, hidden by default; shown by
        // its eye, an override with p1 its consumer. p1 suppressed: kept. c1 newly built from
        // s1: dropped, hidden. Shown again; c1 gone: kept (a loss), its consumers p1 alone;
        // c1 back: dropped (newly built from it again). Unlisted, its override kept as left.
        // The open dialog making c1, or one cancelled, its read stale: no gain, till the
        // timeline drops it. A body's override never dropped.
        let id = |text: &str| FeatureId::new(text).unwrap();
        let s1 = Hideable::Feature(id("s0000001"));
        let sketch = || feature("s0000001", "sketch", false, &[]);
        let pad = |suppressed: bool| feature("p0000001", "pad", suppressed, &["s0000001"]);
        let cut = || feature("c0000001", "pocket", false, &["p0000001", "s0000001"]);
        let take = |tab: &mut Tab, features: Vec<Entry>| {
            tab.timeline = Some(TimelineAnswer {
                bodies: vec![id("p0000001")],
                ..timeline(0, None, features)
            });
            tab.rebrowse();
        };
        let mut tab = tab();
        take(&mut tab, vec![sketch(), pad(false)]);
        assert!(!tab.shown(&s1));
        tab.toggle_shown(s1.clone());
        assert!(tab.shown(&s1));
        take(&mut tab, vec![sketch(), pad(true)]);
        assert!(tab.shown(&s1), "a consumer suppressed drops nothing");
        take(&mut tab, vec![sketch(), pad(false), cut()]);
        assert!(!tab.shown(&s1), "a gain drops it");
        assert!(tab.overrides.is_empty());
        tab.toggle_shown(s1.clone());
        take(&mut tab, vec![sketch(), pad(false)]);
        assert!(tab.shown(&s1), "a loss drops nothing");
        assert_eq!(
            tab.overrides[&s1].under,
            std::collections::BTreeSet::from([id("p0000001")])
        );
        take(&mut tab, vec![sketch(), pad(false), cut()]);
        assert!(!tab.shown(&s1), "lost, then gained again");
        tab.toggle_shown(s1.clone());
        take(&mut tab, vec![pad(false)]);
        assert_eq!(tab.overrides.get(&s1).map(|kept| kept.shown), Some(true));
        take(&mut tab, vec![sketch(), pad(false), cut()]);
        assert!(tab.shown(&s1), "listed again as it was left");
        // The dialog making c1 (its add in the timeline): no gain till it closes with OK.
        let mut tab = self::tab();
        take(&mut tab, vec![sketch(), pad(false)]);
        tab.toggle_shown(s1.clone());
        let mut making = Dialog::new(Kind::Pocket, Some(id("s0000001")), 1);
        making.feature = Some(id("c0000001"));
        assert!(making.made);
        tab.dialog = Some(making.clone());
        take(&mut tab, vec![sketch(), pad(false), cut()]);
        assert!(tab.shown(&s1), "the dialog's feature counts from OK");
        tab.dialog = None;
        tab.rebrowse();
        assert!(!tab.shown(&s1), "OK: a gain");
        // Cancelled instead, its undo landing at revision 3: a read listing c1 gains nothing
        // till then, while its Cancel is unsent too; the first at or past it drops the
        // exclusion, whatever it lists, so c1 made again by a redo (listed at 4) is a gain.
        let at = |n: u64| (1..n).fold(Revision::OPENED, |r, _| r.next().unwrap());
        let take_at = |tab: &mut Tab, revision: Revision, features: Vec<Entry>| {
            tab.timeline = Some(TimelineAnswer {
                revision,
                bodies: vec![id("p0000001")],
                ..timeline(0, None, features)
            });
            tab.rebrowse();
        };
        tab.toggle_shown(s1.clone());
        take_at(&mut tab, at(1), vec![sketch(), pad(false)]);
        tab.dialog = Some(making.clone());
        take_at(&mut tab, at(2), vec![sketch(), pad(false), cut()]);
        tab.dialog = None;
        tab.dropped(Some(&making));
        take_at(&mut tab, at(2), vec![sketch(), pad(false), cut()]);
        assert!(tab.shown(&s1), "its Cancel unsent: a stale read");
        tab.land(Lands::Dialog(1), at(3));
        take_at(&mut tab, at(2), vec![sketch(), pad(false), cut()]);
        assert!(tab.shown(&s1), "older than its undo: a stale read");
        assert_eq!(tab.unmade.len(), 1);
        take_at(&mut tab, at(4), vec![sketch(), pad(false), cut()]);
        assert!(!tab.shown(&s1), "redone past its undo: a gain");
        assert!(tab.unmade.is_empty());
        // Two cancelled, c1 (dialog 1) then c2 (dialog 2), neither undo read yet: a stale
        // read listing either gains nothing; each lands only by its own dialog's Cancel, and
        // counts again from where that lands.
        let c2 = || feature("c0000002", "pocket", false, &["p0000001", "s0000001"]);
        let mut second = Dialog::new(Kind::Pocket, Some(id("s0000001")), 2);
        second.feature = Some(id("c0000002"));
        let mut tab = self::tab();
        take_at(&mut tab, at(1), vec![sketch(), pad(false)]);
        tab.toggle_shown(s1.clone());
        tab.dropped(Some(&making));
        tab.dropped(Some(&second));
        tab.land(Lands::Dialog(1), at(3));
        let landings = |tab: &Tab| -> Vec<Landing> {
            tab.unmade.iter().map(|unmade| unmade.landing).collect()
        };
        assert_eq!(landings(&tab), [Landing::At(at(3)), Landing::Unsent]);
        tab.land(Lands::Dialog(2), at(5));
        take_at(&mut tab, at(2), vec![sketch(), pad(false), cut()]);
        assert!(tab.shown(&s1), "c1's stale read");
        take_at(&mut tab, at(4), vec![sketch(), pad(false), c2()]);
        assert!(tab.shown(&s1), "c2's stale read, past c1's undo");
        assert_eq!(tab.unmade.len(), 1);
        take_at(&mut tab, at(5), vec![sketch(), pad(false)]);
        assert!(tab.unmade.is_empty());
        assert!(tab.shown(&s1));
        // A body's override: never dropped.
        let body = Hideable::Feature(id("p0000001"));
        tab.toggle_shown(body.clone());
        take(&mut tab, vec![sketch(), pad(true), cut()]);
        assert!(!tab.shown(&body));
    }

    #[test]
    fn an_edit_feature_dialog_and_its_cancel_till_its_undo_change_no_override() {
        // Decision 29, "Edit Feature … drop nothing": s1 shown by its eye, p1 built from it.
        // An Edit Feature dialog (made: false) moves p1 onto s2: s1's override, and its
        // consumers p1, kept. Closed, its Cancel unsent, then landing at 3: a read at 2 of the
        // moved state changes nothing still. At 3, p1 back on s1: kept, the freeze ended.
        let id = |text: &str| FeatureId::new(text).unwrap();
        let s1 = Hideable::Feature(id("s0000001"));
        let at = |n: u64| (1..n).fold(Revision::OPENED, |r, _| r.next().unwrap());
        let sketch = |name: &str| feature(name, "sketch", false, &[]);
        let pad_of = |of: &str| feature("p0000001", "pad", false, &[of]);
        let take_at = |tab: &mut Tab, revision: Revision, features: Vec<Entry>| {
            tab.timeline = Some(TimelineAnswer {
                revision,
                bodies: vec![id("p0000001")],
                ..timeline(0, None, features)
            });
            tab.rebrowse();
        };
        let kept = |tab: &Tab| {
            tab.overrides
                .get(&s1)
                .map(|kept| (kept.shown, kept.under.clone()))
                == Some((true, std::collections::BTreeSet::from([id("p0000001")])))
        };
        let moved = || vec![sketch("s0000001"), sketch("s0000002"), pad_of("s0000002")];
        let mut tab = tab();
        take_at(
            &mut tab,
            at(1),
            vec![sketch("s0000001"), pad_of("s0000001")],
        );
        tab.toggle_shown(s1.clone());
        assert!(kept(&tab));
        let mut editing = Dialog::new(Kind::Pad, Some(id("s0000001")), 3);
        editing.made = false;
        editing.feature = Some(id("p0000001"));
        tab.dialog = Some(editing.clone());
        take_at(&mut tab, at(2), moved());
        assert!(kept(&tab), "while the dialog is open");
        tab.dialog = None;
        tab.dropped(Some(&editing));
        assert_eq!(tab.edit_undone, Some((3, Landing::Unsent)));
        take_at(&mut tab, at(2), moved());
        assert!(kept(&tab), "its Cancel unsent");
        tab.land(Lands::Dialog(3), at(3));
        take_at(&mut tab, at(2), moved());
        assert!(kept(&tab), "older than its undo");
        assert_eq!(tab.edit_undone, Some((3, Landing::At(at(3)))));
        take_at(
            &mut tab,
            at(3),
            vec![sketch("s0000001"), pad_of("s0000001")],
        );
        assert!(kept(&tab), "back where it was");
        assert_eq!(tab.edit_undone, None);
        assert!(tab.shown(&s1));
    }

    #[test]
    fn a_cancel_lands_where_its_undo_answered_and_only_its_own_dialogs() {
        // Dialog 4's add answered after it closed (Cancel before the add's answer): c1 unmade,
        // unsent. Dialog 6 still open: its add's answer is its own. Dialog 4's undo answered
        // with the session at 5, though the tab was at 2 (another client's edit at 3 came
        // between): it lands at 5, not 3, and dialog 6's nothing. A revert lands every one
        // left; an answer naming no session lands at the tab's revision.
        let id = |text: &str| FeatureId::new(text).unwrap();
        let at = |n: u64| (1..n).fold(Revision::OPENED, |r, _| r.next().unwrap());
        let info = |revision: Revision| SessionInfo {
            id: SessionId::parse("s1").unwrap(),
            path: None,
            revision,
            transaction: true,
            reloaded: false,
        };
        let mut tab = tab();
        tab.revision = at(2);
        tab.add_answered(4, id("c0000001"));
        let open = Dialog::new(Kind::Pocket, Some(id("s0000001")), 6);
        tab.dialog = Some(open);
        tab.add_answered(6, id("c0000002"));
        assert_eq!(
            tab.dialog
                .as_ref()
                .and_then(|dialog| dialog.feature.clone()),
            Some(id("c0000002"))
        );
        assert_eq!(
            tab.unmade,
            [Unmade {
                feature: id("c0000001"),
                serial: 4,
                landing: Landing::Unsent,
            }]
        );
        tab.add_answered(4, id("c0000001"));
        assert_eq!(tab.unmade.len(), 1, "once");
        tab.dialog = None;
        tab.dropped(Some(&{
            let mut made = Dialog::new(Kind::Pocket, Some(id("s0000001")), 6);
            made.feature = Some(id("c0000002"));
            made
        }));
        tab.landed(Lands::Dialog(4), Some(&info(at(5))));
        let landings = |tab: &Tab| -> Vec<Landing> {
            tab.unmade.iter().map(|unmade| unmade.landing).collect()
        };
        assert_eq!(landings(&tab), [Landing::At(at(5)), Landing::Unsent]);
        tab.landed(Lands::Nothing, Some(&info(at(7))));
        assert_eq!(landings(&tab), [Landing::At(at(5)), Landing::Unsent]);
        tab.landed(Lands::Everything, None);
        assert_eq!(landings(&tab), [Landing::At(at(5)), Landing::At(at(2))]);
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

    /// `entry` with its health `health`.
    fn health(mut entry: Entry, health: Health) -> Entry {
        entry.health = health;
        entry
    }

    #[test]
    fn what_a_feature_consumes_is_named_by_a_feature_other_than_a_sketch_each_by_its_consumer() {
        // A box, a cylinder, a sketch on the box's face, a fuse of the two, and a pad from the
        // sketch: the fuse consumes both bodies, the pad the fuse. The sketch, which names the
        // box, consumes nothing, and the pad doesn't consume it (#443's review, L1: the gate
        // lets any number of features use a sketch, validate.rs's "It isn't consumed").
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let entries = [
            feature("b0000001", "box", false, &[]),
            feature("c0000001", "cylinder", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("z0000001", "fuse", false, &["b0000001", "c0000001"]),
            feature("p0000001", "pad", false, &["z0000001", "s0000001"]),
        ];
        let by: Vec<(FeatureId, FeatureId)> = consumed(&entries)
            .into_iter()
            .map(|(what, by)| (what.clone(), by.clone()))
            .collect();
        assert_eq!(
            by,
            [
                (fid("b0000001"), fid("z0000001")),
                (fid("c0000001"), fid("z0000001")),
                (fid("z0000001"), fid("p0000001")),
            ]
        );
    }

    #[test]
    fn a_sweep_from_a_sketch_on_a_face_builds_on_that_faces_body_where_the_tip_is_another() {
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let sketch = FeatureId::new("s0000001").unwrap();
        // #438's review: a box, a separate cylinder, and a sketch on the box's face. The tip is
        // the cylinder, built last; a sweep from the sketch builds on the box, whose face it's
        // on.
        let two = [
            feature("b0000001", "box", false, &[]),
            feature("c0000001", "cylinder", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
        ];
        assert_eq!(tip_of(&two), id("c0000001"));
        assert_eq!(base_of(&two, Some(&sketch)).unwrap(), id("b0000001"));
        // The box since cut by the cylinder, and another box built after: the base is the cut,
        // where the box's chain ends; the tip, the other box.
        let cut = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
            feature("x0000001", "cut", false, &["b0000001", "c0000001"]),
            feature("d0000001", "box", false, &[]),
        ];
        assert_eq!(tip_of(&cut), id("d0000001"));
        assert_eq!(base_of(&cut, Some(&sketch)).unwrap(), id("x0000001"));
        // The box since consumed by a fuse with the cylinder, as its tool, and another box
        // after: the fuse.
        let fused = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
            feature("z0000001", "fuse", false, &["c0000001", "b0000001"]),
            feature("d0000001", "box", false, &[]),
        ];
        assert_eq!(tip_of(&fused), id("d0000001"));
        assert_eq!(base_of(&fused, Some(&sketch)).unwrap(), id("z0000001"));
        // A chain of two: a fillet of the box, then a chamfer of the fillet, the cylinder
        // after: the chamfer.
        let chain = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("f0000001", "fillet", false, &["b0000001"]),
            feature("f0000002", "chamfer", false, &["f0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(tip_of(&chain), id("c0000001"));
        assert_eq!(base_of(&chain, Some(&sketch)).unwrap(), id("f0000002"));
        // #443's review, L2: a feature that isn't a sketch, though its input is the box, is
        // on no face: the tip.
        assert_eq!(
            base_of(&chain, Some(&FeatureId::new("f0000001").unwrap())).unwrap(),
            id("c0000001")
        );
    }

    #[test]
    fn a_sweeps_walk_to_its_faces_body_follows_what_stands_as_the_tip_does() {
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let sketch = FeatureId::new("s0000001").unwrap();
        // A suppressed fillet of the box, the box's face sketched on: the fillet stands for the
        // box (its chain passes it through), and ends the box's chain; the tip is the cylinder.
        let suppressed = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("f0000001", "fillet", true, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(tip_of(&suppressed), id("c0000001"));
        assert_eq!(base_of(&suppressed, Some(&sketch)).unwrap(), id("f0000001"));
        // The box the tool of a suppressed fuse whose base is a suppressed box: the fuse
        // passes only its base through, standing for nothing, and the core lists the box as
        // a body of its own (#443's review, L3); but the gate counts the fuse as the box's
        // consumer, suppressed or not, so no feature may take the box as its base: a refusal,
        // not a base, and never the tip in its place (#443's review, M2 and L5).
        let tool = [
            feature("e0000001", "box", true, &[]),
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
            feature("z0000001", "fuse", true, &["e0000001", "b0000001"]),
        ];
        assert_eq!(tip_of(&tool), id("c0000001"));
        assert_eq!(
            base_of(&tool, Some(&sketch)),
            Err(
                "b0000001 is the tool of z0000001, which consumes it while suppressed: \
                 unsuppress or remove z0000001 to build on it"
                    .to_owned()
            )
        );
        // The box suppressed itself, with no base to pass through: it stands for nothing, so
        // the chain's end is no body, and the base is the tip, the cylinder.
        let nothing = [
            feature("b0000001", "box", true, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(tip_of(&nothing), id("c0000001"));
        assert_eq!(base_of(&nothing, Some(&sketch)).unwrap(), id("c0000001"));
    }

    #[test]
    fn a_sweep_whose_sketch_has_no_face_body_found_builds_on_the_tip() {
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let sketch = FeatureId::new("s0000001").unwrap();
        let box_ = FeatureId::new("b0000001").unwrap();
        // On an origin plane (no input): the tip.
        let origin = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &[]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(base_of(&origin, Some(&sketch)).unwrap(), id("c0000001"));
        // No sketch named, or a sketch the timeline doesn't list, or a feature that isn't a
        // sketch: the tip.
        let on_box = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(base_of(&on_box, Some(&sketch)).unwrap(), id("b0000001"));
        assert_eq!(base_of(&on_box, None).unwrap(), id("c0000001"));
        let unlisted = FeatureId::new("s0000009").unwrap();
        assert_eq!(base_of(&on_box, Some(&unlisted)).unwrap(), id("c0000001"));
        assert_eq!(base_of(&on_box, Some(&box_)).unwrap(), id("c0000001"));
        // Its face's feature removed (the timeline lists no a0000001): the tip.
        let removed = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["a0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(base_of(&removed, Some(&sketch)).unwrap(), id("c0000001"));
        // Nothing stands at all: none.
        assert_eq!(base_of(&[], Some(&sketch)).unwrap(), None);
    }

    #[test]
    fn a_sweep_from_a_sketch_in_error_on_a_face_builds_on_that_faces_body() {
        // #443's review, M1: a sketch in error (its face not found, `reference_broken`; a
        // conflict; blocked upstream; drift) still names its face's feature, which the
        // document records as the intent, and whose chain ends at a body the gate takes. So
        // a sweep from it builds there, as from one with warnings, or one not built yet
        // (unknown), and as the sketch, once fixed, would have it: never the tip, the
        // cylinder.
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let sketch = FeatureId::new("s0000001").unwrap();
        let on_box = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
        ];
        assert_eq!(tip_of(&on_box), id("c0000001"));
        for kept in [Health::Error, Health::Warning, Health::Unknown] {
            let mut entries = on_box.clone();
            entries[1] = health(entries[1].clone(), kept);
            assert_eq!(
                base_of(&entries, Some(&sketch)).unwrap(),
                id("b0000001"),
                "{kept:?}"
            );
        }
    }

    #[test]
    fn a_sweeps_base_worked_out_again_leaves_the_sweep_itself_out() {
        // #443's review, H1: Block, Pin, Loose on xy, Sketch2 on the Block's top, and the open
        // dialog's Pocket, added from Sketch2 on the Block. Loose picked into it: the tip
        // without the pocket, the Pin (with it, the pocket itself would stand last). Sketch2
        // again, or a sketch on the Block's face: the Block, which the pocket no longer counts
        // as consuming.
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let entries = [
            feature("k7f2qx3m", "box", false, &[]),
            feature("a91cw0de", "cylinder", false, &[]),
            feature("s0000009", "sketch", false, &[]),
            feature("s0000002", "sketch", false, &["k7f2qx3m"]),
            feature("p0000001", "pocket", false, &["k7f2qx3m", "s0000002"]),
        ];
        assert_eq!(tip_of(&entries), id("p0000001"));
        let own = fid("p0000001");
        assert_eq!(
            rebased(&entries, &own, &fid("s0000009")).unwrap(),
            id("a91cw0de")
        );
        assert_eq!(
            rebased(&entries, &own, &fid("s0000002")).unwrap(),
            id("k7f2qx3m")
        );
    }

    /// Block, Pin, Loose (s0000009, on xy), Sketch2 (s0000002, on the Block's face), and
    /// Joined, a fuse of the Pin and the Block, suppressed or not: suppressed, the gate counts
    /// it as the Block's consumer, so no feature may take the Block as its base.
    fn joined(suppressed: bool) -> Vec<Entry> {
        vec![
            feature("k7f2qx3m", "box", false, &[]),
            feature("a91cw0de", "cylinder", false, &[]),
            feature("s0000009", "sketch", false, &[]),
            feature("s0000002", "sketch", false, &["k7f2qx3m"]),
            feature("j0000001", "fuse", suppressed, &["a91cw0de", "k7f2qx3m"]),
        ]
    }

    #[test]
    fn a_reads_refusal_lifted_by_a_newer_timeline_sends_the_add_or_keeps_ok_off() {
        // #443's review: a Pocket's dialog from Sketch2, its add refused by its own read at
        // revision 3 (T1). Against the tab's timeline of revision 2 (T0, older, Joined
        // unsuppressed, which takes the base) the refusal stands, frame after frame. Once the
        // timeline is of revision 3 or newer the check decides: of T1's document (Joined
        // suppressed), refused by the check itself; of a newer one that takes the base (T2,
        // Joined unsuppressed again), lifted, and the add, forgotten when refused, is sent.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let revision = |n: u64| Revision::from_number(n).unwrap();
        let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000002")), 1);
        let members = dialog.add().unwrap();
        assert!(dialog.adding);
        let mut tab = tab();
        tab.dialog = Some(dialog);
        let refusing = TimelineAnswer {
            revision: revision(3),
            ..timeline(0, Some(revision(2)), joined(true))
        };
        tab.base_read(1, Kind::Pocket, members, Ok(refusing));
        assert!(tab.edits.queue.is_empty(), "nothing added");
        let dialog = tab.dialog.as_mut().unwrap();
        assert!(!dialog.adding);
        let refused = dialog.base_refused.clone().unwrap();
        assert_eq!(
            (refused.sketch.as_str(), refused.read),
            ("s0000002", Some(revision(3)))
        );
        // T0, older: the read's refusal stands.
        for _ in 0..3 {
            assert!(base_refused_before_add(
                dialog,
                Some(&joined(false)),
                Some(revision(2))
            ));
            assert_eq!(dialog.base_refused.as_ref(), Some(&refused));
        }
        // T1 itself: the check decides, and refuses, its own refusal now.
        assert!(base_refused_before_add(
            dialog,
            Some(&joined(true)),
            Some(revision(3))
        ));
        assert_eq!(dialog.base_refused.as_ref().unwrap().read, None);
        // T2, newer, taking the base: lifted, and the add sent again.
        dialog.base_refused = Some(refused.clone());
        assert!(!base_refused_before_add(
            dialog,
            Some(&joined(false)),
            Some(revision(4))
        ));
        assert_eq!(dialog.base_refused, None);
        assert!(dialog.add().is_some(), "the refused add sent again");
        // The read's refusal of another sketch than the dialog's now: the check decides.
        let mut other = Dialog::new(Kind::Pocket, Some(fid("s0000009")), 2);
        other.base_refused = Some(refused);
        assert!(!base_refused_before_add(
            &mut other,
            Some(&joined(false)),
            Some(revision(2))
        ));
        assert_eq!(other.base_refused, None);
    }

    /// A session on the fake kernel, its document a box, and a tab of it: what
    /// `dialog_changed` sends to, as a frame's tab does.
    fn session_tab() -> (
        tempfile::TempDir,
        Arc<Sessions<Arc<silico_core::fake::FakeKernel>>>,
        Tab,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(Sessions::with_cache_opener(kernel, |dir| {
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

    #[test]
    fn dialog_changed_keeps_a_reads_refusal_till_the_tabs_timeline_reaches_the_read() {
        // #443's review (round 5, M8): through `dialog_changed`, as each frame asks it. A
        // Pocket's dialog from Sketch2, its add's own read at revision 3 refusing the Block
        // (Joined suppressed there). The tab's timeline at revision 2 (Joined unsuppressed, the
        // Block taken), the session already at 5: frame after frame the read's refusal
        // stands, nothing queued, nothing read, no add. The timeline at 4, taking the Block:
        // lifted, and the add sent again. The timeline at 3 itself, refusing the Block: the
        // check's own refusal, not the read's.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let revision = |n: u64| Revision::from_number(n).unwrap();
        let at = |n: u64, suppressed: bool| TimelineAnswer {
            revision: revision(n),
            ..timeline(0, Some(revision(n - 1)), joined(suppressed))
        };
        let refused_by_the_read = |tab: &mut Tab| {
            let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000002")), 1);
            let members = dialog.add().unwrap();
            tab.dialog = Some(dialog);
            tab.base_read(1, Kind::Pocket, members, Ok(at(3, true)));
        };
        let (_dir, sessions, mut tab) = session_tab();
        tab.revision = revision(5);
        tab.timeline = Some(at(2, false));
        refused_by_the_read(&mut tab);
        for _ in 0..3 {
            assert!(!tab.dialog_changed(&sessions));
            let dialog = tab.dialog.as_ref().unwrap();
            assert_eq!(
                dialog.base_refused.as_ref().map(|refused| refused.read),
                Some(Some(revision(3)))
            );
            assert!(!dialog.adding);
            assert!(tab.edits.queue.is_empty());
            assert!(tab.under.is_empty());
        }
        tab.timeline = Some(at(4, false));
        tab.dialog_changed(&sessions);
        let dialog = tab.dialog.as_ref().unwrap();
        assert_eq!(dialog.base_refused, None);
        assert!(dialog.adding, "the add sent again");
        // The timeline at the read's own revision: the check decides, and refuses.
        let (_dir, sessions, mut tab) = session_tab();
        tab.revision = revision(5);
        tab.timeline = Some(at(2, false));
        refused_by_the_read(&mut tab);
        tab.timeline = Some(at(3, true));
        assert!(!tab.dialog_changed(&sessions));
        let dialog = tab.dialog.as_ref().unwrap();
        assert_eq!(
            dialog.base_refused.as_ref().map(|refused| refused.read),
            Some(None)
        );
        assert!(!dialog.adding);
    }

    #[test]
    fn a_base_edit_in_flight_holds_a_sweeps_ok_and_one_of_another_field_or_feature_doesnt() {
        // #443's mutants (tab.rs:425): an edit in flight counts as one queued does. The
        // Pocket's own `base` in flight holds its OK; its `sketch`, or another feature's
        // `base`, doesn't.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (_dir, sessions, mut tab) = session_tab();
        let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000009")), 1);
        dialog.add().unwrap();
        dialog.added(fid("q0000001"));
        tab.dialog = Some(dialog);
        assert!(tab.can_ok());
        let value = || silico_core::document::json::read(br#""k7f2qx3m""#).unwrap();
        for (feature, field, held) in [
            ("q0000001", "base", true),
            ("q0000001", "sketch", false),
            ("q0000009", "base", false),
        ] {
            tab.edits.in_flight = Some(Flying {
                field: (fid(feature), field.to_owned()),
                value: value(),
                sending: Sending {
                    against: tab.revision,
                    state: tab.revision,
                    step: None,
                },
                running: sessions.set_field(
                    &tab.id,
                    tab.revision,
                    fid(feature),
                    field.to_owned(),
                    value(),
                ),
            });
            assert_eq!(tab.base_held(), held, "{feature}'s {field}");
            assert_eq!(tab.can_ok(), !held, "{feature}'s {field}");
        }
    }

    #[test]
    fn a_new_sweeps_check_before_its_add_isnt_asked_once_its_added() {
        // #443's mutants (tab.rs:4842, `made && sweeps && feature.is_none()`): a Pocket from
        // Sketch2, added; the tab's timeline then one whose Block no feature may take (Joined
        // suppressed). Its Length changed: the edit is sent, the check before the add, which
        // would refuse the Block, not asked of a feature already made.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (_dir, sessions, mut tab) = session_tab();
        let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000002")), 1);
        dialog.add().unwrap();
        dialog.added(fid("q0000001"));
        let length = dialog
            .inputs
            .iter_mut()
            .find(|input| input.label == "Length")
            .unwrap();
        length.value = crate::dialog::Value::Length("7".to_owned());
        tab.dialog = Some(dialog);
        tab.timeline = Some(timeline(0, None, joined(true)));
        assert!(base_of(&joined(true), Some(&fid("s0000002"))).is_err());
        tab.dialog_changed(&sessions);
        assert_eq!(tab.dialog.as_ref().unwrap().base_refused, None);
        assert!(
            tab.edits.in_flight.is_some() || !tab.edits.queue.is_empty(),
            "the Length's edit sent"
        );
    }

    #[test]
    fn a_new_features_add_refused_leaves_ok_off_and_a_stale_one_is_sent_again() {
        // #443's review (round 5, L16; L20): a Pocket's add refused by the core. Not stale (an
        // invalid input, or a conflict over something else than the revision, such as the
        // file's path): OK off (no feature to OK), nothing sent again till an input changes.
        // Stale (a conflict on the revision: made against one the session moved on from): OK
        // off too, and its add sent on the next pass.
        use silico_core::ErrorCode::{Conflict, InvalidInput};
        let fid = |text: &str| FeatureId::new(text).unwrap();
        for (code, parameter, stale) in [
            (InvalidInput, None, false),
            (Conflict, Some("path"), false),
            (Conflict, None, false),
            (InvalidInput, Some("revision"), false),
            (Conflict, Some("revision"), true),
        ] {
            let error = silico_core::Error::new(code, "refused");
            let refusal = Problems::from(match parameter {
                Some(parameter) => error.with_parameter(parameter),
                None => error,
            });
            let mut tab = tab();
            let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000009")), 4);
            dialog.add().unwrap();
            tab.dialog = Some(dialog);
            assert!(!tab.can_ok(), "adding");
            tab.add_refused(4, &refusal);
            let case = format!("{code:?} on {parameter:?}");
            assert!(!tab.can_ok(), "{case}");
            let dialog = tab.dialog.as_mut().unwrap();
            assert!(!dialog.adding, "{case}");
            assert_eq!(dialog.add().is_some(), stale, "{case}");
            assert_eq!(
                tab.message.as_deref().map(|said| said.contains("refused")),
                Some(true)
            );
        }
    }

    #[test]
    fn a_reads_refusal_is_marked_as_the_reads_and_outlasts_an_older_timelines_check() {
        // #443's review: the refusal the add's own read gives is the read's (`read`, its
        // revision), so the check against the tab's older timeline, which takes the base,
        // doesn't clear it; marked as the check's own, it would.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let revision = |n: u64| Revision::from_number(n).unwrap();
        let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000002")), 7);
        let members = dialog.add().unwrap();
        let mut tab = tab();
        tab.dialog = Some(dialog);
        tab.base_read(
            7,
            Kind::Pocket,
            members,
            Ok(TimelineAnswer {
                revision: revision(5),
                ..timeline(0, Some(revision(4)), joined(true))
            }),
        );
        let dialog = tab.dialog.as_mut().unwrap();
        assert!(dialog.base_refused.as_ref().unwrap().read.is_some());
        assert!(base_refused_before_add(
            dialog,
            Some(&joined(false)),
            Some(revision(4))
        ));
        assert!(dialog.base_refused.is_some());
        // Another dialog's read answering changes nothing of this one.
        let before = tab.dialog.clone();
        let theirs = Dialog::new(Kind::Pocket, Some(fid("s0000002")), 8)
            .add()
            .unwrap();
        tab.base_read(8, Kind::Pocket, theirs, Ok(timeline(0, None, joined(true))));
        assert_eq!(tab.dialog, before);
        assert!(tab.edits.queue.is_empty());
    }

    #[test]
    fn a_sweeps_ok_is_held_while_its_base_edit_waits_and_once_its_refused() {
        // #443's review: a new Pocket's dialog, added (its feature p0000001), valid with its
        // sketch. Its own edit of its base queued: OK off; a sketch edit queued alone: on;
        // the base edit refused: off. A boolean's dialog's base edit queued holds nothing
        // here (its box holds it).
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let pocket = fid("p0000001");
        let edit = |field: &str| {
            Queued::Edit(
                Edit {
                    feature: fid("p0000001"),
                    field: field.to_owned(),
                    value: silico_core::document::json::read(b"\"s0000001\"").unwrap(),
                },
                Some(Words::Text("Edit Pocket".to_owned())),
            )
        };
        let mut tab = tab();
        let mut dialog = Dialog::new(Kind::Pocket, Some(fid("s0000001")), 1);
        dialog.feature = Some(pocket.clone());
        tab.dialog = Some(dialog);
        assert!(!tab.base_held());
        tab.edits.queue.push_back(edit("sketch"));
        assert!(!tab.base_held());
        assert!(tab.can_ok());
        tab.edits.queue.push_back(edit("base"));
        assert!(tab.base_held());
        assert!(!tab.can_ok());
        tab.edits.queue.clear();
        assert!(!tab.base_held());
        tab.refused.insert((pocket.clone(), "base".to_owned()));
        assert!(tab.base_held());
        assert!(!tab.can_ok());
        let mut union = Dialog::new(Kind::Union, None, 2);
        union.feature = Some(pocket);
        tab.dialog = Some(union);
        tab.refused.clear();
        tab.edits.queue.push_back(edit("base"));
        assert!(!tab.base_held());
    }

    #[test]
    fn a_sweeps_walk_passes_a_suppressed_consumer_only_from_its_base() {
        // #443's review, L3: a suppressed feature passes only its base through
        // (`document::suppression`). A box b, a sketch on b, a cylinder c, and a suppressed
        // fuse z of base c and tool b: z stands for c; b, its tool, the core lists as a body
        // of its own, but the gate counts z as b's consumer, so no feature may build on b:
        // the sweep's base is refused, saying why by the features' names (M2 and L5).
        let id = |text: &str| Some(FeatureId::new(text).unwrap());
        let sketch = FeatureId::new("s0000001").unwrap();
        let tool = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
            feature("z0000001", "fuse", true, &["c0000001", "b0000001"]),
        ];
        assert_eq!(
            base_of(&tool, Some(&sketch)),
            Err(
                "b0000001 is the tool of z0000001, which consumes it while suppressed: \
                 unsuppress or remove z0000001 to build on it"
                    .to_owned()
            )
        );
        // Named, the names say it.
        let mut named = tool.to_vec();
        named[0].name = "Block".to_owned();
        named[3].name = "Joined".to_owned();
        assert_eq!(
            base_of(&named, Some(&sketch)),
            Err(
                "Block is the tool of Joined, which consumes it while suppressed: unsuppress \
                 or remove Joined to build on it"
                    .to_owned()
            )
        );
        // b the suppressed fuse's base: passed through, to the fuse, which stands for it.
        let base = [
            feature("b0000001", "box", false, &[]),
            feature("s0000001", "sketch", false, &["b0000001"]),
            feature("c0000001", "cylinder", false, &[]),
            feature("z0000001", "fuse", true, &["b0000001", "c0000001"]),
        ];
        assert_eq!(base_of(&base, Some(&sketch)).unwrap(), id("z0000001"));
        // And on through what consumes the fuse: a fillet of it.
        let mut on = base.to_vec();
        on.push(feature("f0000001", "fillet", false, &["z0000001"]));
        assert_eq!(base_of(&on, Some(&sketch)).unwrap(), id("f0000001"));
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
            words: Words::Text("Edit Pad".to_owned()),
        });
        tab.steps.insert(
            two,
            Step {
                words: Words::Text("Edit Pad".to_owned()),
                from: one,
            },
        );
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
        assert_eq!(tab.steps.len(), 1);
        assert_eq!(tab.restored.len(), 1);
        tab.ended(Ok(()), false, &mut close);
        assert!(tab.groups.is_empty());
        assert!(tab.steps.is_empty());
        assert!(tab.restored.is_empty());
    }

    fn rev(number: u64) -> Revision {
        Revision::from_number(number).unwrap()
    }

    /// The timeline listing a block, a pin, and a sketch, each by the name the person sees.
    fn named_timeline() -> TimelineAnswer {
        let named = |id: &str, name: &str| Entry {
            name: name.to_owned(),
            ..entry(id, 0, 0)
        };
        timeline(
            0,
            None,
            vec![
                named("a0000001", "Block"),
                named("b0000001", "Pin"),
                named("s0000001", "Triangle"),
            ],
        )
    }

    /// The tab at `revision`, read as the status of it counts `undo` and `redo` steps.
    fn stood_at(tab: &mut Tab, revision: Revision, undo: usize, redo: usize) {
        tab.revision = revision;
        tab.status_revision = revision;
        tab.status = Some(counted(revision, true, undo, redo));
    }

    /// A change sent at `against`, the session's state then `state`, named `step`, applied
    /// at `made` (the answer's revision, as the session gives it).
    fn apply(tab: &mut Tab, against: Revision, state: Revision, step: &str, made: Revision) {
        let sending = Sending {
            against,
            state,
            step: Some(Words::Text(step.to_owned())),
        };
        tab.applied(&sending, &answered_at(made, Ok(())));
    }

    fn answered_at(revision: Revision, result: Result<(), Problems>) -> Response<()> {
        Response {
            session: Some(silico_core::command::SessionInfo {
                id: SessionId::parse("s1").unwrap(),
                path: Some("a.silico".to_owned()),
                revision,
                transaction: true,
                reloaded: false,
            }),
            model: None,
            result,
        }
    }

    #[test]
    fn each_change_sent_says_what_it_does_to_which_feature_and_whats_answered_nothing() {
        // The prototype's step labels (app.js and interact.js: `'Suppress '`/`'Unsuppress ' +
        // featName`, `'Edit ' + name`) in its grammar, a verb and the feature's name, for each
        // kind of change the tab sends and says as it sends it; Restore by its banner's
        // button (Fable's decision of 2026-11-04). A deletion and an add say nothing as
        // they're sent: their answers name them, as a rename's does. What changes nothing is
        // no step.
        let fid = |id: &str| FeatureId::new(id).unwrap();
        let mut tab = tab();
        tab.took_timeline(named_timeline());
        let pin = fid("b0000001");
        let sketch = fid("s0000001");
        let curve = ElementId::new("na000000").unwrap();
        let value = || silico_core::document::json::read(b"5").unwrap();
        let cases: Vec<(Queued, Option<&str>)> = vec![
            (
                Queued::Move {
                    feature: pin.clone(),
                    before: None,
                },
                Some("Move Pin"),
            ),
            (
                Queued::Suppress {
                    feature: pin.clone(),
                    suppressed: true,
                },
                Some("Suppress Pin"),
            ),
            (
                Queued::Suppress {
                    feature: pin.clone(),
                    suppressed: false,
                },
                Some("Unsuppress Pin"),
            ),
            (
                Queued::Remove {
                    feature: fid("a0000001"),
                },
                None,
            ),
            (
                Queued::Add {
                    serial: 1,
                    kind: Kind::Pad,
                    members: Vec::new(),
                    body: Body::Unread,
                },
                None,
            ),
            (
                Queued::Add {
                    serial: 1,
                    kind: Kind::Subtract,
                    members: Vec::new(),
                    body: Body::Unread,
                },
                None,
            ),
            (
                Queued::AddSketch {
                    serial: 2,
                    feature: value(),
                },
                None,
            ),
            (
                Queued::SketchTrim {
                    sketch: sketch.clone(),
                    curve: curve.clone(),
                    pick: [0.0, 0.0],
                },
                Some("Edit Triangle"),
            ),
            (
                Queued::SketchRemove {
                    sketch: sketch.clone(),
                    members: Vec::new(),
                },
                Some("Edit Triangle"),
            ),
            (
                Queued::DimensionValue {
                    serial: 2,
                    sketch: sketch.clone(),
                    constraint: curve.clone(),
                    value: value(),
                },
                Some("Edit Triangle"),
            ),
            (
                Queued::Offer {
                    serial: 2,
                    request: OfferRequest::SetField {
                        revision: rev(3),
                        feature: sketch.clone(),
                        field: "constraints.c0000001.driving".to_owned(),
                        value: value(),
                    },
                },
                Some("Edit Triangle"),
            ),
            (
                Queued::Replace {
                    revision: rev(4),
                    text: String::new(),
                },
                Some("Restore revision 4"),
            ),
            (
                Queued::Edit(
                    Edit {
                        feature: pin.clone(),
                        field: "radius".to_owned(),
                        value: value(),
                    },
                    Some(Words::Of("Edit", pin.clone())),
                ),
                Some("Edit Pin"),
            ),
            (
                Queued::Edit(
                    Edit {
                        feature: pin.clone(),
                        field: "name".to_owned(),
                        value: name_value("PinA").unwrap(),
                    },
                    None,
                ),
                None,
            ),
            (
                Queued::Offer {
                    serial: 2,
                    request: OfferRequest::Undo {
                        revision: rev(3),
                        to: None,
                    },
                },
                None,
            ),
            (Queued::Save { then_close: false }, None),
            (
                Queued::Step {
                    undo: true,
                    floor: None,
                    sketch: None,
                },
                None,
            ),
            (
                Queued::Closed {
                    ok: true,
                    sketch: false,
                    dialog: Some(1),
                    words: Some(Words::Text("Edit Pad".to_owned())),
                },
                None,
            ),
            (Queued::Restore { revision: rev(4) }, None),
        ];
        let one = Revision::OPENED;
        for (queued, text) in cases {
            let words = tab.step_words(&queued);
            assert_eq!(
                words
                    .as_ref()
                    .and_then(|words| tab.step_text(words, one, one))
                    .as_deref(),
                text,
                "{queued:?}"
            );
        }
        // Sketch mode's own steps: by its sketch, once its add has answered; Create
        // Sketch's before that by the name its add gives it; with neither, "Sketch", as the
        // sketch bar says it.
        tab.sketch = Some(SketchMode::new(3, Some(sketch.clone())));
        assert_eq!(tab.sketch_words("Edit"), Words::Of("Edit", sketch));
        let mut mode = SketchMode::new(4, None);
        mode.created = Some("Sketch2".to_owned());
        tab.sketch = Some(mode);
        assert_eq!(
            tab.sketch_words("Add"),
            Words::Text("Add Sketch2".to_owned())
        );
        tab.sketch = Some(SketchMode::new(5, None));
        assert_eq!(
            tab.sketch_words("Edit"),
            Words::Text("Edit Sketch".to_owned())
        );
    }

    #[test]
    fn an_edit_is_named_by_the_dialog_or_sketch_its_made_in_and_a_rename_only_by_its_answer() {
        // What an edit asked says, decided as it's asked (its feature named once its step is
        // made, from the state it's made from: `step_text`; a rename's by its own answer).
        let fid = |id: &str| FeatureId::new(id).unwrap();
        let mut tab = tab();
        tab.took_timeline(named_timeline());
        let one = Revision::OPENED;
        let pin = fid("b0000001");
        let name = |text: &str| Edit {
            feature: pin.clone(),
            field: "name".to_owned(),
            value: name_value(text).unwrap(),
        };
        let radius = Edit {
            feature: pin.clone(),
            field: "radius".to_owned(),
            value: silico_core::document::json::read(b"3").unwrap(),
        };
        let text = |tab: &Tab, edit: &Edit| {
            tab.edit_words(edit)
                .and_then(|words| tab.step_text(&words, one, one))
        };
        // A rename says nothing as it's asked, wherever: its answer names it. Anything else
        // outside a dialog, "Edit" and the feature.
        assert_eq!(tab.edit_words(&name("PinA")), None);
        assert_eq!(text(&tab, &radius).as_deref(), Some("Edit Pin"));
        // Two edits of a field with nothing between them are one: the second takes the first's
        // place, keeping what it says.
        let mut edits = Edits::default();
        let height = |number: &str| Edit {
            feature: pin.clone(),
            field: "height".to_owned(),
            value: silico_core::document::json::read(number.as_bytes()).unwrap(),
        };
        edits.push(height("4"), Some(Words::Text("Edit Pin".to_owned())));
        edits.push(height("5"), Some(Words::Text("Edit Peg".to_owned())));
        assert_eq!(edits.queue.len(), 1);
        assert!(matches!(
            &edits.queue[0],
            Queued::Edit(edit, Some(Words::Text(words)))
                if edit.value == height("5").value && words == "Edit Pin"
        ));
        // A new feature's dialog: its edits, and its OK's one step, by its kind's title till
        // its add answers; then by its feature.
        tab.dialog = Some(Dialog::new(Kind::Pad, None, 1));
        assert_eq!(text(&tab, &radius).as_deref(), Some("Edit Pad"));
        assert_eq!(
            tab.dialog_step(tab.dialog.as_ref().unwrap()),
            Words::Text("Add Pad".to_owned())
        );
        // An existing feature's (Edit Feature's): "Edit" and it; a rename made in it says
        // nothing as it's asked, its answer naming it, as anywhere.
        let mut editing = Dialog::new(Kind::Cylinder, None, 2);
        editing.made = false;
        editing.feature = Some(pin.clone());
        assert_eq!(tab.dialog_step(&editing), Words::Of("Edit", pin.clone()));
        tab.dialog = Some(editing);
        assert_eq!(tab.edit_words(&name("PinA")), None);
        tab.dialog = None;
        // Sketch mode's: "Edit" and the sketch; a rename made in it, a rename still.
        tab.sketch = Some(SketchMode::new(3, Some(fid("s0000001"))));
        assert_eq!(text(&tab, &radius).as_deref(), Some("Edit Triangle"));
        assert_eq!(tab.edit_words(&name("PinA")), None);
        // A name past the window's width, or across lines, as the tooltip shows it: on one
        // line, cut at 60 characters with an ellipsis.
        tab.sketch = None;
        let mut long = named_timeline();
        if let Some(features) = long.features.as_mut() {
            features[1].name = format!("Pin\n\t {}", "W".repeat(70));
        }
        tab.took_timeline(long);
        assert_eq!(
            text(&tab, &radius),
            Some(format!("Edit Pin {}…", "W".repeat(56)))
        );
    }

    #[test]
    fn undo_and_redo_name_the_steps_either_side_of_the_sessions_state() {
        // Worked by hand. Revision 1 is the state `begin` saw. Pin renamed (2); a new pad's
        // dialog, its add and one edit (3, 4), OK'd as one step, "Add Pad"; the pin
        // suppressed (5).
        let mut tab = tab();
        let one = Revision::OPENED;
        apply(&mut tab, one, one, "Rename Pin", rev(2));
        apply(&mut tab, rev(2), rev(2), "Add Pad", rev(3));
        apply(&mut tab, rev(3), rev(3), "Edit Pad", rev(4));
        tab.groups.push(Group {
            start: rev(2),
            end: rev(4),
            words: Words::Text("Add Pad".to_owned()),
        });
        apply(&mut tab, rev(4), rev(4), "Suppress Pin", rev(5));
        // At 5, the core counting four edits to undo: Undo takes back the suppression;
        // nothing to redo; three unsaved edits (the pad's two one step).
        stood_at(&mut tab, rev(5), 4, 0);
        assert_eq!(tab.undo_step().as_deref(), Some("Suppress Pin"));
        assert_eq!(tab.redo_step().as_deref(), None);
        assert_eq!(tab.unsaved_edits(), 3);
        // Undone (6 restores 4's state): Undo takes the pad's two edits back as one; Redo
        // gives the suppression back.
        tab.restored.insert(rev(6), rev(4));
        stood_at(&mut tab, rev(6), 3, 1);
        assert_eq!(tab.undo_step().as_deref(), Some("Add Pad"));
        assert_eq!(tab.redo_step().as_deref(), Some("Suppress Pin"));
        assert_eq!(tab.unsaved_edits(), 2);
        // The pad undone whole (7 restores 2's): Undo the rename; Redo the pad, whole.
        tab.restored.insert(rev(7), rev(2));
        stood_at(&mut tab, rev(7), 1, 3);
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.redo_step().as_deref(), Some("Add Pad"));
        assert_eq!(tab.unsaved_edits(), 1);
        // Nothing of the tab's answered yet (an edit waiting): neither names a step, its
        // answer moving what each would take.
        tab.edits.push(
            Edit {
                feature: FeatureId::new("b0000001").unwrap(),
                field: "radius".to_owned(),
                value: silico_core::document::json::read(b"3").unwrap(),
            },
            Some(Words::Text("Edit Pin".to_owned())),
        );
        assert!(tab.undoable());
        assert_eq!(tab.undo_step().as_deref(), None);
        tab.edits = Edits::default();
        // A change from 2 (applied at 8) drops the redo branch: the pad's steps, its group,
        // and the suppression.
        apply(&mut tab, rev(7), rev(2), "Delete Hole", rev(8));
        assert!(tab.groups.is_empty());
        stood_at(&mut tab, rev(8), 2, 0);
        assert_eq!(tab.undo_step().as_deref(), Some("Delete Hole"));
        assert_eq!(tab.redo_step().as_deref(), None);
        assert_eq!(tab.unsaved_edits(), 2);
        // Undone (9 restores 2's), then again (10 restores 1's, the state `begin` saw):
        // nothing left to undo, so Undo names none; Redo the rename; nothing unsaved.
        tab.restored.insert(rev(9), rev(2));
        stood_at(&mut tab, rev(9), 1, 1);
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.redo_step().as_deref(), Some("Delete Hole"));
        assert_eq!(tab.unsaved_edits(), 1);
        tab.restored.insert(rev(10), one);
        stood_at(&mut tab, rev(10), 0, 2);
        assert_eq!(tab.undo_step().as_deref(), None);
        assert_eq!(tab.redo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.unsaved_edits(), 0);
    }

    #[test]
    fn undo_and_redo_name_nothing_while_off_or_while_anything_of_the_tabs_waits() {
        // The pin and the hole renamed (2, 3), one undone (4 restores 2's): Undo names the pin's
        // rename, Redo the hole's. A dialog open turns Undo off (its Cancel undoes its edits): it
        // names nothing. A move queued leaves Redo on (it waits behind it), but names nothing, the
        // move's answer dropping what it would give back.
        let mut tab = tab();
        let one = Revision::OPENED;
        apply(&mut tab, one, one, "Rename Pin", rev(2));
        apply(&mut tab, rev(2), rev(2), "Rename Hole", rev(3));
        tab.restored.insert(rev(4), rev(2));
        stood_at(&mut tab, rev(4), 1, 1);
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.redo_step().as_deref(), Some("Rename Hole"));
        tab.dialog = Some(Dialog::new(Kind::Pad, None, 1));
        assert!(!tab.undoable());
        assert_eq!(tab.undo_step().as_deref(), None);
        tab.dialog = None;
        tab.edits.queue.push_back(Queued::Move {
            feature: FeatureId::new("a0000001").unwrap(),
            before: None,
        });
        assert!(tab.redoable());
        assert_eq!(tab.redo_step().as_deref(), None);
    }

    #[test]
    fn a_steps_feature_is_named_in_the_state_it_was_made_from_else_in_the_one_it_made() {
        // Names by hand: at 1 the pin is Pin; at 2 it's Peg; at 3 it's Pin and the pad, made there,
        // is Pad; 4 and 5 aren't known.
        let mut tab = tab();
        let pin = FeatureId::new("b0000001").unwrap();
        let pad = FeatureId::new("p0000001").unwrap();
        let names = |pairs: &[(&FeatureId, &str)]| Names {
            listed: Arc::new(
                pairs
                    .iter()
                    .map(|(feature, name)| ((*feature).clone(), (*name).to_owned()))
                    .collect(),
            ),
            since: BTreeMap::new(),
        };
        tab.names.insert(rev(1), names(&[(&pin, "Pin")]));
        tab.names.insert(rev(2), names(&[(&pin, "Peg")]));
        tab.names
            .insert(rev(3), names(&[(&pin, "Pin"), (&pad, "Pad")]));
        let text = |words: Words, from: u64, made: u64| tab.step_text(&words, rev(from), rev(made));
        // From 1 to 2: named as it was at 1, not as it is at 2.
        assert_eq!(
            text(Words::Of("Edit", pin.clone()), 1, 2).as_deref(),
            Some("Edit Pin")
        );
        assert_eq!(
            text(Words::Then(pin.clone(), "remove its edge"), 2, 3).as_deref(),
            Some("Peg: remove its edge")
        );
        // From 1 to 3: the pad not there at 1, so as it is at 3, which made it.
        assert_eq!(
            text(Words::Of("Add", pad), 1, 3).as_deref(),
            Some("Add Pad")
        );
        // From 4 to 5: known in neither, so nothing.
        assert_eq!(text(Words::Of("Edit", pin), 4, 5), None);
    }

    #[test]
    fn a_change_refused_or_changing_nothing_names_no_state_and_keeps_the_redo_branch() {
        let mut tab = tab();
        let one = Revision::OPENED;
        apply(&mut tab, one, one, "Rename Pin", rev(2));
        apply(&mut tab, rev(2), rev(2), "Rename Hole", rev(3));
        // Undone to 2's state (at 4).
        tab.restored.insert(rev(4), rev(2));
        stood_at(&mut tab, rev(4), 1, 1);
        assert_eq!(tab.redo_step().as_deref(), Some("Rename Hole"));
        // Refused: nothing named, the redo kept.
        let refused = Problems::from(silico_core::Error::new(
            silico_core::ErrorCode::InvalidInput,
            "no",
        ));
        let sending = Sending {
            against: rev(4),
            state: rev(2),
            step: Some(Words::Text("Delete Block".to_owned())),
        };
        tab.applied(&sending, &answered_at(rev(4), Err(refused)));
        // Changing nothing: answered at the revision it was sent against.
        tab.applied(&sending, &answered_at(rev(4), Ok(())));
        let step = |text: &str, from: Revision| Step {
            words: Words::Text(text.to_owned()),
            from,
        };
        assert_eq!(
            tab.steps.clone().into_iter().collect::<Vec<_>>(),
            [
                (rev(2), step("Rename Pin", Revision::OPENED)),
                (rev(3), step("Rename Hole", rev(2)))
            ]
        );
        assert_eq!(tab.redo_step().as_deref(), Some("Rename Hole"));
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
    }

    #[test]
    fn the_unsaved_count_takes_a_sketchs_or_a_dialogs_edits_as_one_step_as_its_finish_or_ok_will() {
        // Worked by hand: Pin renamed (2); sketch mode opened at 2 and three edits of its
        // own (3, 4, 5): two unsaved edits, the sketch's one, as its Finish makes them; Undo
        // in it takes back its own last edit.
        let mut tab = tab();
        let one = Revision::OPENED;
        apply(&mut tab, one, one, "Rename Pin", rev(2));
        let mut mode = SketchMode::new(1, Some(FeatureId::new("s0000001").unwrap()));
        mode.opened_at = Some(rev(2));
        tab.sketch = Some(mode);
        for at in 3..=5 {
            apply(&mut tab, rev(at - 1), rev(at - 1), "Edit Triangle", rev(at));
        }
        stood_at(&mut tab, rev(5), 4, 0);
        assert_eq!(tab.unsaved_edits(), 2);
        assert_eq!(tab.undo_step().as_deref(), Some("Edit Triangle"));
        // As a poll keeps it, nothing of the tab's under way.
        tab.held_count = tab.edits_now();
        // Finished: sketch mode gone, its close waiting in the queue, its group not yet made:
        // the count held, two, not the four steps not yet one.
        tab.sketch = None;
        tab.dialog_base = Some(rev(2));
        tab.edits.queue.push_back(Queued::Closed {
            ok: true,
            sketch: true,
            dialog: None,
            words: Some(Words::Text("Edit Triangle".to_owned())),
        });
        assert_eq!(tab.edits_now(), 4);
        assert_eq!(tab.unsaved_edits(), 2);
        // Its close taken: one step, the group's.
        tab.edits.queue.clear();
        tab.dialog_base = None;
        tab.groups.push(Group {
            start: rev(2),
            end: rev(5),
            words: Words::Text("Edit Triangle".to_owned()),
        });
        assert_eq!(tab.unsaved_edits(), 2);
        assert_eq!(tab.undo_step().as_deref(), Some("Edit Triangle"));
        // An edit after it: three.
        apply(&mut tab, rev(5), rev(5), "Rename Block", rev(6));
        stood_at(&mut tab, rev(6), 5, 0);
        assert_eq!(tab.unsaved_edits(), 3);
        // Edit Feature open at 6, two edits of its own (7, 8): four, its edits one step, as
        // its OK will make them.
        let mut editing = Dialog::new(Kind::Cylinder, None, 2);
        editing.made = false;
        editing.feature = Some(FeatureId::new("b0000001").unwrap());
        tab.dialog = Some(editing);
        tab.dialog_base = Some(rev(6));
        apply(&mut tab, rev(6), rev(6), "Edit Pin", rev(7));
        apply(&mut tab, rev(7), rev(7), "Edit Pin", rev(8));
        stood_at(&mut tab, rev(8), 7, 0);
        assert_eq!(tab.unsaved_edits(), 4);
        // Its close asked: an OK leaves four, held while it waits; a Cancel three, its edits
        // taken back.
        tab.dialog = None;
        tab.count_closed(true, rev(6));
        assert_eq!(tab.held_count, 4);
        tab.count_closed(false, rev(6));
        assert_eq!(tab.held_count, 3);
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
            consumers: Default::default(),
            suppressed: false,
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
        tab.browser = Browser::new(vec![row("aaaaaaaa")], Vec::new());
        assert_eq!(tab.why_closed(Kind::Subtract), Some(NEEDS_TWO_BODIES));
        assert_eq!(tab.why_closed(Kind::Box), None);
        tab.browser = Browser::new(vec![row("aaaaaaaa"), row("bbbbbbbb")], Vec::new());
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
            dialog: None,
            words: None,
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
    fn a_timeline_read_sent_with_a_fresh_checks_answer_records_the_model_it_published() {
        // Rebuild everything's fresh check publishes its model, then answers. A poll counts
        // the published models first, then takes the answers, then sends the timeline read
        // the answer calls for: so the publish and the answer can both land between the count
        // and the read. Here they do, on purpose: the check answered (a command queued after
        // it has) while the tab didn't poll, then the poll's steps after its count. The read
        // records the model the check published, the one past those the tab had counted.
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let sessions = Arc::new(Sessions::with_cache_opener(
            Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
            |dir| silico_core::cache::Cache::unsealed(dir),
        ));
        let opened = sessions.open(&document, false).wait();
        let id = opened.session.unwrap().id;
        let mut tab = Tab::opened(
            id,
            document.to_str().unwrap().to_owned(),
            &opened.result.unwrap().problems,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while tab.timeline.is_none() || tab.timeline_read.is_some() || tab.published_models == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the timeline never read"
            );
            tab.poll(&sessions);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let seen = tab.published_models;
        assert!(tab.can_rebuild_all());
        tab.rebuild_all(&sessions);
        sessions.text(&id, None).wait().result.unwrap();
        assert_eq!(tab.published_models, seen);
        tab.take_answers(&sessions);
        tab.read_timeline(&sessions);
        assert!(tab.timeline_read.is_some());
        assert_eq!(tab.timeline_read_after, seen + 1);
        while tab.timeline_read.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the timeline never read again"
            );
            tab.poll(&sessions);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            (tab.timeline_after, tab.published_models),
            (seen + 1, seen + 1)
        );
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

    /// A session of one box, unbuilt, its tab, and every kernel request held from here on;
    /// its rebuild started once by a published read and stopped, so a published read (the
    /// tab's timeline read sends one) starts it no more, and the first read of the current
    /// model starts it itself, as its waiter ([`waiting`](Self::waiting)).
    struct HeldBox {
        _dir: tempfile::TempDir,
        kernel: Arc<silico_core::fake::FakeKernel>,
        sessions: Arc<Sessions<Arc<silico_core::fake::FakeKernel>>>,
        id: SessionId,
        tab: Tab,
        hold: silico_core::fake::Hold,
        /// The boxes asked of the kernel by the stopped rebuild, before any of the tab's.
        stopped_boxes: usize,
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
            // A published read with nothing published starts the rebuild with no command
            // waiting on it (decision 13), and a read cancelled before it reaches its wait
            // stops nothing (decision 34), so a read sent after such a start could be
            // cancelled with the rebuild left running. Stopped, the rebuild is started again
            // only by a read of the current model, which waits on what it starts.
            let _ = sessions.published(&id);
            assert!(kernel.wait_running(1, HELD_WAIT));
            assert!(sessions.cancel(&id, Cancellable::Rebuild).result.is_ok());
            assert!(kernel.wait_idle(HELD_WAIT));
            let stopped_boxes = boxes(&kernel);
            assert_eq!(stopped_boxes, 1);
            HeldBox {
                _dir: dir,
                kernel,
                sessions,
                id,
                tab,
                hold,
                stopped_boxes,
            }
        }

        /// How many times the box was asked of the kernel since the stopped rebuild asked it.
        fn boxes(&self) -> usize {
            boxes(&self.kernel) - self.stopped_boxes
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
        /// the box held: the rebuild running at all says the read started it, and so waits on
        /// it, nothing else starting it ([`HeldBox`]).
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
                    // Of the published model: answered at once, refused, nothing being
                    // published and its rebuild stopped; the read sent next starts it.
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
            let stopped_boxes = held.stopped_boxes;
            let stopped = held.tab.halt_with(&held.sessions, |cancelled| {
                // The waiter cancelled, the other read still uncancelled: the session runs it
                // next, which starts the rebuild again, asking for the box. Counted as
                // `HeldBox::boxes` counts, from the setup's stopped rebuild.
                if cancelled == waiter {
                    until("the stopped rebuild started again", &|| {
                        boxes(&kernel) - stopped_boxes > 1
                    });
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
        let fake = silico_core::fake::FakeKernel::new(1).unwrap();
        kernel(&fake);
        opened_in(
            Sessions::with_cache_opener(Arc::new(fake), |dir| {
                silico_core::cache::Cache::unsealed(dir)
            }),
            text,
        )
    }

    /// `text` written to a file, opened by `sessions` in a tab: its directory (kept for the
    /// session's file), the sessions, and the tab.
    fn opened_in(
        sessions: Sessions<Arc<silico_core::fake::FakeKernel>>,
        text: &str,
    ) -> (tempfile::TempDir, FakeSessions, Tab) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(&document, text).unwrap();
        let sessions = Arc::new(sessions);
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
            tab.browser = Browser::new(
                vec![Row {
                    id: cut.clone(),
                    name: "b0000001".to_owned(),
                    health: Health::Healthy,
                    marked_by: None,
                    consumers: Default::default(),
                    suppressed: false,
                }],
                Vec::new(),
            );
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

    /// `tab` polled until `done` holds, within a minute.
    fn polled_until(tab: &mut Tab, sessions: &FakeSessions, done: impl Fn(&Tab) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            tab.poll(sessions);
            if done(tab) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "never so");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_refused_undo_leaves_what_undo_and_redo_name_and_the_count_as_they_were() {
        // The pin renamed through the session: one step, "Rename Pin" (by hand). Then an
        // undo made against a revision that isn't the session's: refused as a conflict, and
        // said, the session where it was: Undo names the rename still, Redo nothing, and the
        // rename is counted once.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        tab.rename(&sessions, FeatureId::new("a91cw0de").unwrap(), "PinA");
        polled_until(&mut tab, &sessions, |tab| tab.quiet() && tab.undoable());
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.unsaved_edits(), 1);
        tab.under.push(Under::Step {
            running: sessions.undo(&tab.id, rev(99), None),
            undo: true,
            sketch: None,
            offered: false,
            lands: Lands::Nothing,
        });
        polled_until(&mut tab, &sessions, |tab| !tab.stepping());
        assert!(tab.message.is_some(), "the undo wasn't refused");
        polled_until(&mut tab, &sessions, |tab| tab.quiet() && tab.undoable());
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Pin"));
        assert_eq!(tab.redo_step().as_deref(), None);
        assert_eq!(tab.unsaved_edits(), 1);
    }

    /// The session's status read into `tab` as [`Tab::poll`] reads it, without poll's check for
    /// the session's own reload (a move seen with nothing of the tab's under way), and nothing
    /// else of a poll's: no answer taken, no timeline read. Valid with work of the tab's under
    /// way, whose answers the status read may be ahead of.
    fn status_now(tab: &mut Tab, sessions: &FakeSessions) {
        let status = sessions.status(&tab.id);
        let session = status.session.unwrap();
        tab.revision = session.revision;
        tab.transaction = session.transaction;
        tab.status_revision = session.revision;
        tab.status = Some(status.result.unwrap());
    }

    /// BLOCK open, its pin renamed PinA, and the timeline read again, listing it.
    fn pin_renamed() -> (tempfile::TempDir, FakeSessions, Tab, FeatureId) {
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        let pin = pin_renamed_in(&mut tab, &sessions);
        (dir, sessions, tab, pin)
    }

    /// BLOCK, open in `tab`, its pin renamed PinA, and the timeline read again, listing it.
    /// The pin.
    fn pin_renamed_in(tab: &mut Tab, sessions: &FakeSessions) -> FeatureId {
        polled_until(tab, sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.rename(sessions, pin.clone(), "PinA");
        polled_until_listed(tab, sessions);
        assert_eq!(tab.name_of(&pin), "PinA");
        pin
    }

    /// Every answer taken, the timeline not read again, and the status read: what Undo
    /// names then.
    fn undo_named(tab: &mut Tab, sessions: &FakeSessions) -> Option<String> {
        taken_until(tab, sessions, Tab::quiet);
        status_now(tab, sessions);
        tab.undo_step()
    }

    #[test]
    fn a_rename_behind_or_after_an_undo_of_a_rename_replaces_the_name_the_undo_gave_back() {
        // PinA, listed. An undo asked and a rename asked while it runs: the undo gives Pin back, so
        // the rename replaces Pin. Then that undone too, its answer taken but the timeline not read
        // again, and a rename: it replaces Pin too.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.step(&sessions, true);
        assert!(tab.stepping());
        tab.rename(&sessions, pin.clone(), "PinB");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename Pin")
        );
        tab.step(&sessions, true);
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.rename(&sessions, pin, "PinC");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename Pin")
        );
    }

    #[test]
    fn a_suppression_sent_as_an_undo_answers_names_the_name_the_undo_gave_back() {
        // PinA, listed; an undo asked, and the pin suppressed behind it, sent as the undo's answer
        // is taken, before the timeline is read again: it suppresses Pin.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.step(&sessions, true);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Suppress Pin")
        );
    }

    #[test]
    fn a_suppression_after_a_restore_names_the_pin_as_the_restore_put_it_back() {
        // PinA, listed; a Restore of the revision the transaction began at (its text, with Pin, put
        // back: what that state names isn't known till a timeline lists it), and the pin suppressed
        // after. Undo names nothing till a timeline lists the state the suppression made, then
        // "Suppress Pin": never PinA.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.restore(&sessions, Revision::OPENED);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Pin")
        );
    }

    /// Polled until nothing of the tab's is under way and a timeline lists the revision the
    /// session is at.
    fn polled_until_listed(tab: &mut Tab, sessions: &FakeSessions) {
        polled_until(tab, sessions, |tab| {
            tab.quiet()
                && tab
                    .timeline
                    .as_ref()
                    .is_some_and(|timeline| timeline.revision == tab.revision)
        });
    }

    /// Polled until a timeline lists the revision the session is at, nothing of the tab's
    /// under way: what Undo names then.
    fn named_once_listed(tab: &mut Tab, sessions: &FakeSessions) -> Option<String> {
        polled_until_listed(tab, sessions);
        tab.undo_step()
    }

    #[test]
    fn a_suppression_sent_as_a_reverts_rollback_answers_is_named_once_its_state_is_listed() {
        // Names are never carried across a rollback. PinA, listed; a revert, and the pin suppressed
        // behind it, from the state the rollback came to: its names aren't known till a timeline
        // lists one, so Undo names nothing; then, listed, the file's Pin.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.revert(&sessions, false);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Pin")
        );
    }

    #[test]
    fn a_suppression_sent_as_a_saves_answer_is_taken_is_named_once_its_state_is_listed() {
        // Names are never carried across a save. PinA, listed; PinB, a save, and the pin suppressed
        // behind them: nothing named till listed, then PinB.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        tab.save(&sessions, false);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress PinB")
        );
    }

    #[test]
    fn a_suppression_sent_as_a_save_as_answer_is_taken_is_named_once_its_state_is_listed() {
        // As a save, a Save As to another file.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress PinB")
        );
    }

    /// Off any poll, until the session's transaction has closed (a commit's or a rollback's
    /// applied, its answer not yet taken).
    fn ended_off_poll(tab: &Tab, sessions: &FakeSessions) {
        until("the transaction ended", &|| {
            !sessions.status(&tab.id).session.unwrap().transaction
        });
    }

    /// The file at `path` written by another program: BLOCK with its pin named `name`.
    fn pin_named_at(path: &std::path::Path, name: &str) {
        let named = BLOCK.replace(r#""name": "Pin""#, &format!(r#""name": "{name}""#));
        written_by_another_program(path, &named);
    }

    /// `text` written at `path` as another program saves a file, and as Silico saves one
    /// (`silico_core::file::save_over`): whole, to a file beside it renamed over it, so
    /// nothing reads part of it. Written in place, a disk-watch comparison could read it
    /// half-written, and the session reload it twice, the first of part of the file.
    fn written_by_another_program(path: &std::path::Path, text: &str) {
        let beside = path.with_extension("theirs");
        std::fs::write(&beside, text).unwrap();
        std::fs::rename(&beside, path).unwrap();
    }

    /// The session's revision as `status` says it now.
    fn revision_now(tab: &Tab, sessions: &FakeSessions) -> Revision {
        sessions.status(&tab.id).session.unwrap().revision
    }

    /// The session's own reload of its file, written with the pin named Bolt, past the
    /// revision `past`: asked by a read of its text, if the disk watch hasn't had it read
    /// first. The revision it came to.
    fn reloaded_past(tab: &Tab, sessions: &FakeSessions, past: Revision) -> Revision {
        let read = sessions.text(&tab.id, None).wait();
        let revision = read.session.unwrap().revision;
        assert!(revision > past, "the file not reloaded");
        assert!(read.result.unwrap().text.contains(r#""name": "Bolt""#));
        revision
    }

    /// The reload banner's from and to, if one is shown.
    fn banner(tab: &Tab) -> Option<(Revision, Revision)> {
        tab.reloaded
            .as_ref()
            .map(|reloaded| (reloaded.from, reloaded.to))
    }

    /// The reloads the banner shown says replaced theirs before their changes were listed:
    /// one reload announced twice would list itself here, its first banner's diff not yet
    /// answered.
    fn banner_unlisted(tab: &Tab) -> Vec<(Revision, Revision)> {
        tab.reloaded
            .as_ref()
            .map(|reloaded| reloaded.unlisted.clone())
            .unwrap_or_default()
    }

    /// No reload unlisted.
    const NONE_UNLISTED: Vec<(Revision, Revision)> = Vec::new();

    /// The session's status read, then its answers taken, as a poll takes them, but no
    /// timeline read started: what `status` says comes before the answers it may be ahead of.
    fn status_then_answers(tab: &mut Tab, sessions: &FakeSessions) {
        status_now(tab, sessions);
        taken_until(tab, sessions, Tab::quiet);
    }

    #[test]
    fn a_rollback_answered_after_the_sessions_own_reload_lends_the_reload_no_names() {
        // PinA, listed; another program writes the file with the pin named Bolt; a revert, applied,
        // then the session's own reload of the changed file (by its disk watch, or a read of its
        // text, outside a transaction now), its revision read from `status` before the rollback's
        // answer is taken: the reload's banner, from the rollback's revision to the reload's. The
        // pin suppressed then, from the reload's state: nothing named till a timeline lists it,
        // then Bolt.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        tab.revert(&sessions, false);
        ended_off_poll(&tab, &sessions);
        // The rollback's revision, counted, not read from `status`: the disk watch may have
        // reloaded the file already. 1 at open; PinA's begin keeps it and its edit makes 2; the
        // rollback, a new revision of begin's state, makes 3. The reload, whichever of the watch
        // and the text read asks it, comes after.
        let rolled_back = Revision::from_number(3).unwrap();
        let reload = reloaded_past(&tab, &sessions, rolled_back);
        status_then_answers(&mut tab, &sessions);
        assert!(!tab.transaction);
        assert_eq!(banner(&tab), Some((rolled_back, reload)));
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Bolt")
        );
    }

    #[test]
    fn a_saves_answer_taken_after_status_read_its_revision_is_named_once_its_state_is_listed() {
        // PinA, listed; PinB answered; a save, applied, its revision read from `status` before its
        // answer is taken. The pin suppressed then: nothing named till a timeline lists it, then
        // PinB.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        status_then_answers(&mut tab, &sessions);
        assert!(!tab.transaction);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress PinB")
        );
    }

    #[test]
    fn a_saves_answer_taken_after_the_sessions_own_reload_lends_the_reload_no_names() {
        // PinA, listed; PinB answered; a save, applied; another program then writes the file with
        // the pin named Bolt, and the session reloads it, its revision read from `status` before
        // the save's answer is taken: the reload's banner, as a rollback's, announced once. The
        // pin suppressed then, from the reload's state: nothing named till a timeline lists it,
        // then Bolt, never the PinB the save kept.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        // Counted: 1 at open, PinA 2, PinB 3, the save 4; the reload, once the file is written
        // after it, 5.
        let saved = rev(4);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        assert_eq!(reloaded_past(&tab, &sessions, saved), rev(5));
        status_then_answers(&mut tab, &sessions);
        assert_eq!(banner(&tab), Some((saved, rev(5))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Bolt")
        );
    }

    #[test]
    fn a_save_as_answer_taken_after_the_sessions_own_reload_lends_the_reload_no_names() {
        // As the save above, a Save As: it writes another file, and the session, holding that file
        // from then on, reloads it once another program writes it Bolt: its banner, from the Save
        // As's revision to the reload's, announced once.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        ended_off_poll(&tab, &sessions);
        // Counted as the save's: the Save As 4 (inside PinA's transaction, so no begin of its
        // own), the reload 5.
        let saved = rev(4);
        pin_named_at(&copy, "Bolt");
        assert_eq!(reloaded_past(&tab, &sessions, saved), rev(5));
        status_then_answers(&mut tab, &sessions);
        assert_eq!(banner(&tab), Some((saved, rev(5))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Bolt")
        );
    }

    /// PinA listed, PinB answered; then `end` (a save, a Save As, or a revert) asked, the
    /// file as the session holds it: its answer taken with `status` read once it's applied
    /// (`status_first`), or with no status read. The reload banner then.
    fn banner_after(
        status_first: bool,
        end: impl FnOnce(&mut Tab, &FakeSessions, &std::path::Path),
    ) -> Option<(Revision, Revision)> {
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin, "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        end(&mut tab, &sessions, dir.path());
        if status_first {
            ended_off_poll(&tab, &sessions);
            status_now(&mut tab, &sessions);
        }
        taken_until(&mut tab, &sessions, Tab::quiet);
        banner(&tab)
    }

    #[test]
    fn a_reverts_answer_with_no_reload_before_it_shows_no_banner() {
        // The person's own revert, the file unchanged: whether `status` read the revision it
        // came to before its answer was taken or nothing read it, no reload is announced.
        for status_first in [true, false] {
            let shown = banner_after(status_first, |tab, sessions, _| tab.revert(sessions, false));
            assert_eq!(shown, None, "status first: {status_first}");
        }
    }

    #[test]
    fn a_saves_answer_with_no_reload_before_it_shows_no_banner() {
        // As a revert, the person's own save.
        for status_first in [true, false] {
            let shown = banner_after(status_first, |tab, sessions, _| tab.save(sessions, false));
            assert_eq!(shown, None, "status first: {status_first}");
        }
    }

    #[test]
    fn a_save_as_answer_with_no_reload_before_it_shows_no_banner() {
        // As a save, a Save As to another file.
        for status_first in [true, false] {
            let shown = banner_after(status_first, |tab, sessions, dir| {
                let copy = dir.join("copy.silico");
                tab.save_as(sessions, copy.to_str().unwrap().to_owned(), false, false);
            });
            assert_eq!(shown, None, "status first: {status_first}");
        }
    }

    #[test]
    fn a_refused_saves_answer_behind_another_clients_edit_shows_no_banner() {
        // PinA (2), its transaction open; another program writes the file, so a save, which
        // reads it inside the transaction too, is refused, the transaction left open; then
        // another client of the session renames the pin inside it (3), read from `status`
        // before the save's answer is taken. The answer, at 2, is behind the tab, but a
        // session never reloads inside a transaction: no banner.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        assert!(tab.transaction);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        tab.save(&sessions, false);
        let renamed = sessions
            .set_field(
                &tab.id,
                rev(2),
                pin,
                "name".to_owned(),
                silico_core::document::json::read(br#""Nut""#).unwrap(),
            )
            .wait();
        assert_eq!(renamed.session.unwrap().revision, rev(3));
        status_now(&mut tab, &sessions);
        taken_until(&mut tab, &sessions, Tab::quiet);
        assert!(tab.message.is_some(), "the save wasn't refused");
        assert!(tab.transaction);
        assert_eq!(banner(&tab), None);
    }

    #[test]
    fn a_saves_state_listed_before_its_answer_is_taken_keeps_that_listing() {
        // PinA listed; PinB answered; a save, applied, its revision read from `status`, and a
        // timeline of that revision taken before the save's answer is: the answer keeps that
        // listing, so the pin suppressed then is named at once, "Suppress PinB".
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        status_now(&mut tab, &sessions);
        let listed = sessions
            .timeline(&tab.id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        assert_eq!(listed.revision, tab.revision);
        tab.took_timeline(listed);
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Suppress PinB")
        );
    }

    #[test]
    fn a_save_as_whose_own_begin_reloaded_the_file_lends_the_saved_state_no_old_names() {
        // BLOCK open and listed, no transaction; another program writes it with the pin named Bolt;
        // a Save As, whose own `begin` reloads the file first, and the pin suppressed behind it:
        // nothing named till a timeline lists it, then Bolt, never the Pin of the state the Save As
        // was asked at.
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        tab.suppress_feature(&sessions, pin, true);
        assert_eq!(undo_named(&mut tab, &sessions), None);
        assert_eq!(
            named_once_listed(&mut tab, &sessions).as_deref(),
            Some("Suppress Bolt")
        );
    }

    #[test]
    fn a_save_as_whose_own_begin_reloaded_the_file_shows_the_reloads_banner_once() {
        // PinA (2), saved (3), no transaction open; another program writes the file with the
        // pin named Bolt; a Save As, whose own `begin`, sent at 3, reloads the file first (4),
        // the copy then written (5) of what it reloaded: the reload's banner, from 3 to 4,
        // announced once.
        let (dir, sessions, mut tab, _pin) = pin_renamed();
        tab.save(&sessions, false);
        taken_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(tab.revision, rev(3));
        assert!(!tab.transaction);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        taken_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(tab.revision, rev(5));
        assert_eq!(banner(&tab), Some((rev(3), rev(4))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
        let written = std::fs::read_to_string(&copy).unwrap();
        assert!(written.contains(r#""name": "Bolt""#), "{written}");
    }

    /// BLOCK, its pin's radius 0.6, as another program writes it.
    fn pin_radius_six_tenths() -> String {
        BLOCK.replace(r#""radius": 0.5"#, r#""radius": 0.6"#)
    }

    /// `feature`'s `field` set to `value` (JSON), as its Edit Feature dialog sends it.
    fn field_set(feature: &FeatureId, field: &str, value: &str) -> Edit {
        Edit {
            feature: feature.clone(),
            field: field.to_owned(),
            value: silico_core::document::json::read(value.as_bytes()).unwrap(),
        }
    }

    /// BLOCK open and listed (1), the pin's Edit Feature dialog open on it, nothing of the
    /// tab's under way and no transaction open; then another program writes the pin's radius
    /// 0.6, which the session hasn't read yet. The pin.
    fn pins_dialog_over_a_changed_file() -> (tempfile::TempDir, FakeSessions, Tab, FeatureId) {
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.edit_feature(&sessions, &pin);
        polled_until(&mut tab, &sessions, |tab| {
            tab.dialog.is_some() && tab.quiet()
        });
        assert_eq!((tab.revision, tab.transaction), (rev(1), false));
        written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
        (dir, sessions, tab, pin)
    }

    /// BLOCK open and listed (1), the pin's Edit Feature dialog open on it, nothing of the
    /// tab's under way and no transaction open; another program writes the pin's radius 0.6;
    /// then the dialog's first edit, the radius 0.75, sent at 1, its `begin` reloading the
    /// file first (2), so the edit, sent against 1, is refused. Its answers taken by polls
    /// (`polled`), or with no `status` read, so the tab is still at 1 as the begin answers.
    /// The reload's banner, from 1 to 2, announced once; and the dialog, read again, at 2,
    /// its radius the file's, the transaction the begin opened its own.
    fn first_edit_whose_begin_reloads(polled: bool) -> (tempfile::TempDir, FakeSessions, Tab) {
        let (dir, sessions, mut tab, pin) = pins_dialog_over_a_changed_file();
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        let done = |tab: &Tab| tab.quiet() && tab.dialog.is_some();
        if polled {
            polled_until(&mut tab, &sessions, done);
        } else {
            taken_until(&mut tab, &sessions, done);
        }
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
        let said = tab.message.clone().unwrap_or_default();
        assert!(
            said.contains("revision"),
            "the edit wasn't refused: {said:?}"
        );
        assert_eq!(
            tab.dialog
                .as_ref()
                .unwrap()
                .sent("radius")
                .map(String::as_str),
            Some("0.6")
        );
        assert_eq!(tab.dialog_base, Some(rev(2)));
        assert!(sessions.status(&tab.id).session.unwrap().transaction);
        (dir, sessions, tab)
    }

    /// The dialog closed (`ok`), and polled until nothing of the tab's is under way: whether
    /// the session has a transaction open then, and the file's text.
    fn closed_after_the_reload(
        dir: &tempfile::TempDir,
        sessions: &FakeSessions,
        mut tab: Tab,
        ok: bool,
    ) -> (bool, String) {
        tab.close_dialog(sessions, ok);
        polled_until(&mut tab, sessions, Tab::quiet);
        let open = sessions.status(&tab.id).session.unwrap().transaction;
        let text = std::fs::read_to_string(dir.path().join("part.silico")).unwrap();
        (open, text)
    }

    #[test]
    fn edit_features_first_edit_whose_begin_reloads_the_file_then_cancel_leaves_no_transaction() {
        // The dialog's first edit refused after its own begin reloaded the file: the
        // transaction that begin opened is the dialog's, so Cancel rolls it back, and the
        // session follows its file again.
        let (dir, sessions, tab) = first_edit_whose_begin_reloads(true);
        let (open, text) = closed_after_the_reload(&dir, &sessions, tab, false);
        assert!(!open, "Cancel left a transaction open");
        assert_eq!(text, pin_radius_six_tenths());
    }

    #[test]
    fn edit_features_only_edit_refused_after_its_begin_reloaded_the_file_then_ok_leaves_no_transaction()
     {
        // As Cancel, OK: nothing of the dialog's changed the document, so it ends the
        // transaction its own begin opened.
        let (dir, sessions, tab) = first_edit_whose_begin_reloads(true);
        let (open, text) = closed_after_the_reload(&dir, &sessions, tab, true);
        assert!(!open, "OK left a transaction open");
        assert_eq!(text, pin_radius_six_tenths());
    }

    #[test]
    fn a_begins_reload_taken_before_status_read_it_opens_the_dialog_at_the_reload() {
        // The begin's answer taken with the tab still at 1: the dialog opens at the reload's
        // revision, 2, never the tab's, so its OK, nothing of its own changed, ends the
        // transaction.
        let (dir, sessions, tab) = first_edit_whose_begin_reloads(false);
        let (open, _) = closed_after_the_reload(&dir, &sessions, tab, true);
        assert!(!open, "OK left a transaction open");
    }

    #[test]
    fn a_dialogs_two_edits_whose_first_begin_reloads_the_file_are_its_own_to_cancel() {
        // The pin's dialog open at 1; another program writes the radius 0.6; the radius 0.75
        // typed, then the height 4 behind it: the first, its begin reloading the file (2),
        // refused; the second sent after, at 2, applied (3), before the dialog is read
        // again. Both after the reload, so both the dialog's: OK keeps the height, one step
        // from the reload; Cancel rolls the transaction back, the file as the other program
        // wrote it.
        for ok in [true, false] {
            let (dir, sessions, mut tab, pin) = pins_dialog_over_a_changed_file();
            tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
            tab.edit(&sessions, field_set(&pin, "height", "4"));
            polled_until(&mut tab, &sessions, |tab| {
                tab.quiet() && tab.dialog.is_some()
            });
            assert_eq!(banner(&tab), Some((rev(1), rev(2))), "ok: {ok}");
            assert_eq!(tab.revision, rev(3), "ok: {ok}");
            assert_eq!(tab.dialog_base, Some(rev(2)), "ok: {ok}");
            tab.close_dialog(&sessions, ok);
            polled_until(&mut tab, &sessions, Tab::quiet);
            let open = sessions.status(&tab.id).session.unwrap().transaction;
            assert_eq!(open, ok, "ok: {ok}");
            if ok {
                let groups: Vec<_> = tab.groups.iter().map(|g| (g.start, g.end)).collect();
                assert_eq!(groups, [(rev(2), rev(3))]);
            } else {
                let text = std::fs::read_to_string(dir.path().join("part.silico")).unwrap();
                assert_eq!(text, pin_radius_six_tenths());
            }
        }
    }

    #[test]
    fn two_reloads_before_a_saves_answer_show_one_banner_over_both() {
        // A save applied (4); another program writes the file twice, each reloaded (5, 6),
        // before `status` reads the session at 6 and the save's answer, at 4, is taken: the
        // tab saw neither, so one banner, from 4 to 6, with nothing unlisted.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin, "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        let file = dir.path().join("part.silico");
        pin_named_at(&file, "Bolt");
        assert_eq!(reloaded_past(&tab, &sessions, rev(4)), rev(5));
        pin_named_at(&file, "Nut");
        // A read of the document now, outside a transaction: it reloads the file.
        let read = sessions.text(&tab.id, None).wait();
        assert_eq!(read.session.unwrap().revision, rev(6));
        status_then_answers(&mut tab, &sessions);
        assert_eq!(banner(&tab), Some((rev(4), rev(6))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
    }

    #[test]
    fn a_reload_by_a_save_as_begin_and_another_after_it_end_on_the_second_ones_banner() {
        // PinA (2), saved (3); another program writes the file Bolt; a Save As, its own begin
        // reloading it (4), the copy written (5); then another program writes the copy, and
        // the session reloads it (6), read by `status` before either answer is taken. The
        // banner the second's, from 5 to 6; the first listed as replaced only if its diff
        // hadn't answered, and nothing else.
        let (dir, sessions, mut tab, _pin) = pin_renamed();
        tab.save(&sessions, false);
        taken_until(&mut tab, &sessions, Tab::quiet);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        until("the copy written", &|| {
            revision_now(&tab, &sessions) >= rev(5)
        });
        written_by_another_program(&copy, &pin_radius_six_tenths());
        // A read of the document now, outside a transaction: it reloads the file.
        let read = sessions.text(&tab.id, None).wait();
        assert_eq!(read.session.unwrap().revision, rev(6));
        status_then_answers(&mut tab, &sessions);
        assert_eq!(banner(&tab), Some((rev(5), rev(6))));
        let unlisted = banner_unlisted(&tab);
        assert!(
            unlisted.iter().all(|older| *older == (rev(3), rev(4))),
            "{unlisted:?}"
        );
    }

    #[test]
    fn cancel_while_the_dialog_is_read_again_after_its_begins_reload_leaves_no_transaction() {
        // The pin's dialog open at 1; another program writes the radius 0.6; the radius 0.75
        // typed, its begin reloading the file (2): Cancel as soon as the begin's answer is
        // taken, the dialog not yet read again. The transaction that begin opened is still the
        // dialog's: rolled back, none left open.
        let (dir, sessions, mut tab, pin) = pins_dialog_over_a_changed_file();
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        taken_until(&mut tab, &sessions, |tab| {
            !tab.under
                .iter()
                .any(|under| matches!(under, Under::Begin { .. }))
        });
        assert!(tab.dialog.is_none() && tab.loading.is_some());
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        let (open, text) = closed_after_the_reload(&dir, &sessions, tab, false);
        assert!(!open, "Cancel left a transaction open");
        assert_eq!(text, pin_radius_six_tenths());
    }

    #[test]
    fn a_save_as_and_close_whose_own_begin_reloads_under_a_dialog_still_closes() {
        // PinA (2), saved (3), the pin's dialog open, no transaction; another program writes
        // the file; Save As and close (an untitled part's Close), its own begin reloading the
        // file (4) under the dialog, which is read again, queued: the copy written, the tab
        // closes, nothing queued sent into a new transaction.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.save(&sessions, false);
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.edit_feature(&sessions, &pin);
        polled_until(&mut tab, &sessions, |tab| {
            tab.dialog.is_some() && tab.quiet()
        });
        assert!(!tab.transaction);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, true);
        taken_until(&mut tab, &sessions, |tab| tab.closed || tab.close_refused);
        assert!(!tab.take_close_refused(), "{:?}", tab.message);
        assert!(tab.closed);
        let written = std::fs::read_to_string(&copy).unwrap();
        assert!(written.contains(r#""name": "Bolt""#), "{written}");
    }

    #[test]
    fn ok_after_the_session_answered_its_edit_whose_begin_reloads_leaves_no_transaction() {
        // The pin's dialog open at 1 over a changed file; the radius 0.75 typed, its begin
        // reloading the file (2) and the edit, against 1, refused, both answered before OK is
        // pressed, so OK's advance takes the edit's answer before any take of the begin's.
        // The close waits for the begin's answer, which moves it to the reload: nothing of
        // the dialog's changed, so the transaction that begin opened is rolled back, and no
        // step is made of the reload.
        let (_dir, sessions, mut tab, pin) = pins_dialog_over_a_changed_file();
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        answered_so_far(&tab, &sessions);
        tab.close_dialog(&sessions, true);
        polled_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_closed_with_nothing(&tab, &sessions, "OK");
    }

    #[test]
    fn ok_with_the_begins_answer_untaken_as_advance_runs_leaves_no_transaction() {
        // As above, OK pressed first: the session answers the begin and the refused edit
        // only after, and an advance takes the edit's answer with the begin's still untaken.
        let (_dir, sessions, mut tab, pin) = pins_dialog_over_a_changed_file();
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        tab.close_dialog(&sessions, true);
        answered_so_far(&tab, &sessions);
        tab.advance(&sessions);
        polled_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_closed_with_nothing(&tab, &sessions, "OK");
    }

    #[test]
    fn ok_after_a_save_ended_the_transaction_it_opened_in_whose_edits_begin_reloads() {
        // PinA (2), the pin's dialog opened inside its transaction, so not its own; saved (3),
        // the transaction ended; another program writes the radius; the radius 0.75 typed,
        // its begin reloading the file (4), both answered before OK. The close, queued as the
        // reload is seen, opens at the reload with the transaction since its own: rolled back,
        // no step made.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.edit_feature(&sessions, &pin);
        polled_until(&mut tab, &sessions, |tab| {
            tab.dialog.is_some() && tab.quiet()
        });
        assert!(tab.transaction);
        assert!(!tab.dialog_began);
        tab.save(&sessions, false);
        polled_until(&mut tab, &sessions, |tab| tab.quiet() && !tab.transaction);
        assert_eq!(tab.revision, rev(3));
        let groups_before = tab.groups.len();
        written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        answered_so_far(&tab, &sessions);
        tab.close_dialog(&sessions, true);
        polled_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(banner(&tab).map(|(from, _)| from), Some(rev(3)));
        let open = sessions.status(&tab.id).session.unwrap().transaction;
        assert!(!open, "OK left an empty transaction open");
        assert_eq!(tab.groups.len(), groups_before);
    }

    /// BLOCK and a sketch of one point, Loose (`s0000001`).
    fn block_and_loose() -> String {
        let loose = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Loose", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
        BLOCK.replace("}]}", &format!("}}, {loose}]}}"))
    }

    /// `text` open in a tab, as [`opened_on`] opens it, its sketches solved by the fake.
    fn opened_sketching(text: &str) -> (tempfile::TempDir, FakeSessions, Tab) {
        let sketches = silico_core::fake::FakeSketches::new();
        opened_in(
            Sessions::with_stand_ins(
                Arc::new(silico_core::fake::FakeKernel::new(1).unwrap()),
                |dir| silico_core::cache::Cache::unsealed(dir),
                move || Ok(Box::new(sketches.clone()) as Box<dyn silico_core::sketch::SketchSolver>),
            ),
            text,
        )
    }

    #[test]
    fn finish_after_the_session_answered_its_edit_whose_begin_reloads_leaves_no_transaction() {
        // Sketch mode open on Loose at 1, nothing of its own sent; another program writes the
        // pin's radius; an edit sent, its begin reloading the file (2) and the edit, against 1,
        // refused, both answered before Finish, so sketch mode has gone, its close queued, as
        // the begin's answer is taken. As a dialog's OK: the transaction rolled back, no step
        // made.
        let text = block_and_loose();
        let (dir, sessions, mut tab) = opened_sketching(&text);
        polled_until(&mut tab, &sessions, |tab| {
            tab.timeline.is_some() && tab.quiet()
        });
        let loose = FeatureId::new("s0000001").unwrap();
        tab.edit_sketch(&sessions, &loose);
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.sketch.as_ref().is_some_and(|mode| mode.read.is_some())
        });
        assert_eq!((tab.revision, tab.transaction), (rev(1), false));
        let theirs = text.replace(r#""radius": 0.5"#, r#""radius": 0.6"#);
        written_by_another_program(&dir.path().join("part.silico"), &theirs);
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
        answered_so_far(&tab, &sessions);
        tab.finish_sketch(&sessions, true);
        polled_until(&mut tab, &sessions, Tab::quiet);
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_closed_with_nothing(&tab, &sessions, "Finish");
    }

    #[test]
    fn a_reload_while_edit_features_read_is_out_shows_its_banner() {
        // BLOCK at 1, quiet; another program writes the pin's radius; Edit Feature asked, its
        // read of the feature, outside a transaction, reading the file first and reloading it
        // (2): the reload's banner, announced once, and the dialog's radius the file's.
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| {
            tab.timeline.is_some() && tab.quiet()
        });
        assert_eq!((tab.revision, tab.transaction), (rev(1), false));
        written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
        let resets = tab.resets;
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.edit_feature(&sessions, &pin);
        polled_until(&mut tab, &sessions, |tab| {
            tab.dialog.is_some() && tab.quiet()
        });
        for _ in 0..10 {
            tab.poll(&sessions);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tab.revision, rev(2), "the file not reloaded");
        assert_eq!(
            tab.dialog
                .as_ref()
                .unwrap()
                .sent("radius")
                .map(String::as_str),
            Some("0.6")
        );
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_eq!(
            tab.resets,
            resets + 1,
            "the reload announced more than once"
        );
    }

    #[test]
    fn a_reload_seen_by_both_edit_features_read_and_a_begin_is_announced_and_read_once() {
        // BLOCK at 1, quiet; another program writes the pin's radius; Edit Feature asked, its
        // read sent at 1, and in the same frame the block renamed, its begin sent at 1 too and
        // queued behind the read, which reloads the file (2) first. Both answers say the
        // session moved past 1, taken together: one reload, announced once, and the dialog
        // the read's answer opens read no second time.
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| {
            tab.timeline.is_some() && tab.quiet()
        });
        written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
        let resets = tab.resets;
        tab.edit_feature(&sessions, &FeatureId::new("a91cw0de").unwrap());
        let opened = tab.loading;
        tab.rename(&sessions, FeatureId::new("k7f2qx3m").unwrap(), "Slab");
        answered_so_far(&tab, &sessions);
        taken_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.dialog.is_some()
        });
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_eq!(
            tab.resets,
            resets + 1,
            "the reload announced more than once"
        );
        assert_eq!(tab.dialog.as_ref().map(|dialog| dialog.serial), opened);
    }

    #[test]
    fn a_reload_said_by_the_read_then_seen_by_a_poll_reads_the_dialog_once() {
        // Edit Feature's read reloads the file and says so; its answer moves the dialog, not
        // the tab, whose next poll finds the session past it: that poll says nothing and
        // reads the dialog no second time.
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| {
            tab.timeline.is_some() && tab.quiet()
        });
        written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
        tab.edit_feature(&sessions, &FeatureId::new("a91cw0de").unwrap());
        let opened = tab.loading;
        taken_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.dialog.is_some()
        });
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        assert_eq!(
            tab.revision,
            rev(1),
            "precondition: the tab behind the session"
        );
        let resets = tab.resets;
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.dialog.is_some()
        });
        for _ in 0..5 {
            tab.poll(&sessions);
        }
        assert_eq!(tab.revision, rev(2));
        assert_eq!(tab.resets, resets);
        assert_eq!(tab.dialog.as_ref().map(|dialog| dialog.serial), opened);
    }

    #[test]
    fn a_reload_by_edit_features_read_again_after_a_saves_reload_shows_its_banner() {
        // PinA (2), the pin's dialog open; a save, or a Save As, applied (3); another program
        // writes the file, and the session reloads it (4), read from `status` before the
        // answer: its banner as the answer is taken, the dialog's read queued again. The
        // session's queue held, so that read waits, and another program writes the file again
        // before it runs: its read reloads it (5), and that reload has its banner too.
        use std::sync::atomic::Ordering;
        for save_as in [false, true] {
            let (dir, sessions, mut tab, holds) = opened_holding(BLOCK, 1);
            let (holding, hold) = &holds[0];
            let pin = pin_renamed_in(&mut tab, &sessions);
            tab.edit_feature(&sessions, &pin);
            polled_until(&mut tab, &sessions, |tab| {
                tab.dialog.is_some() && tab.quiet()
            });
            let file = if save_as {
                let copy = dir.path().join("copy.silico");
                tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
                copy
            } else {
                tab.save(&sessions, false);
                dir.path().join("part.silico")
            };
            ended_off_poll(&tab, &sessions);
            pin_named_at(&file, "Bolt");
            assert_eq!(reloaded_past(&tab, &sessions, rev(3)), rev(4));
            status_now(&mut tab, &sessions);
            holding.store(true, Ordering::SeqCst);
            let check = held_check(&tab, &sessions);
            taken_until(&mut tab, &sessions, |tab| {
                tab.under
                    .iter()
                    .any(|under| matches!(under, Under::Load { .. }))
            });
            assert_eq!(banner(&tab), Some((rev(3), rev(4))), "save as: {save_as}");
            pin_named_at(&file, "Nut");
            hold.release();
            let _ = check.wait();
            // Polled until the tab is at the session's revision too: the read's answer moves the
            // dialog, not the tab's revision, which the poll after takes from `status`.
            polled_until(&mut tab, &sessions, |tab| {
                tab.dialog.is_some() && tab.quiet() && tab.revision == rev(5)
            });
            assert_eq!(tab.revision, rev(5), "save as: {save_as}");
            assert_eq!(revision_now(&tab, &sessions), rev(5), "save as: {save_as}");
            assert_eq!(banner(&tab), Some((rev(4), rev(5))), "save as: {save_as}");
            assert_eq!(banner_unlisted(&tab), NONE_UNLISTED, "save as: {save_as}");
        }
    }

    #[test]
    fn edit_feature_opened_inside_a_transaction_then_saved_and_reloaded_cancel_leaves_none() {
        // PinA (2), its transaction open, then the pin's dialog opened inside it, so not its
        // own; saved (3), the transaction ended; another program writes the radius and the
        // session reloads it (4), seen by a poll, or read from `status` before the save's
        // answer and seen as that answer is taken. The dialog read again, opening there, no
        // transaction open, so the transaction its next edit begins is its own, and Cancel
        // rolls it back.
        for before_the_answer in [false, true] {
            let (dir, sessions, mut tab, pin) = pin_renamed();
            tab.edit_feature(&sessions, &pin);
            polled_until(&mut tab, &sessions, |tab| {
                tab.dialog.is_some() && tab.quiet()
            });
            assert!(tab.transaction);
            assert!(!tab.dialog_began);
            tab.save(&sessions, false);
            if before_the_answer {
                ended_off_poll(&tab, &sessions);
            } else {
                polled_until(&mut tab, &sessions, |tab| tab.quiet() && !tab.transaction);
                assert_eq!(tab.revision, rev(3));
            }
            written_by_another_program(&dir.path().join("part.silico"), &pin_radius_six_tenths());
            // A read of the document now, outside a transaction: it reloads the file.
            let reloaded = sessions
                .text(&tab.id, None)
                .wait()
                .session
                .unwrap()
                .revision;
            assert_eq!(reloaded, rev(4), "before the answer: {before_the_answer}");
            if before_the_answer {
                status_then_answers(&mut tab, &sessions);
            }
            polled_until(&mut tab, &sessions, |tab| {
                tab.quiet() && radius_shown(tab) == Some("0.6")
            });
            assert_eq!(
                banner(&tab),
                Some((rev(3), rev(4))),
                "before the answer: {before_the_answer}"
            );
            cancel_after_an_edit(dir, sessions, tab, &pin, reloaded);
        }
    }

    /// Waited, off any poll, until the session has answered everything the tab sent before
    /// now: it answers in turn, so a read of its text asked now answers only after them. Each
    /// use asks it after a `begin`, inside whose transaction it reads nothing of the file.
    fn answered_so_far(tab: &Tab, sessions: &FakeSessions) {
        let _ = sessions.text(&tab.id, None).wait();
    }

    /// Once a dialog's OK or sketch mode's Finish (`closed`) is taken: no transaction left
    /// open, and no step made of the reload.
    fn assert_closed_with_nothing(tab: &Tab, sessions: &FakeSessions, closed: &str) {
        let groups: Vec<_> = tab.groups.iter().map(|g| (g.start, g.end)).collect();
        let open = sessions.status(&tab.id).session.unwrap().transaction;
        assert!(
            !open,
            "{closed} left an empty transaction open; groups {groups:?}"
        );
        assert_eq!(groups, Vec::<(Revision, Revision)>::new());
    }

    /// The open dialog's radius, as the document holds it as far as it knows.
    fn radius_shown(tab: &Tab) -> Option<&str> {
        tab.dialog.as_ref()?.sent("radius").map(String::as_str)
    }

    /// The pin's radius 0.75 sent, applied past `reloaded` in a transaction it begins; then
    /// Cancel: no transaction left open, and the file as the other program wrote it.
    fn cancel_after_an_edit(
        dir: tempfile::TempDir,
        sessions: FakeSessions,
        mut tab: Tab,
        pin: &FeatureId,
        reloaded: Revision,
    ) {
        tab.edit(&sessions, field_set(pin, "radius", "0.75"));
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.revision > reloaded
        });
        assert!(tab.transaction);
        tab.close_dialog(&sessions, false);
        polled_until(&mut tab, &sessions, Tab::quiet);
        let open = sessions.status(&tab.id).session.unwrap().transaction;
        assert!(!open, "Cancel left a transaction open");
        let text = std::fs::read_to_string(dir.path().join("part.silico")).unwrap();
        assert_eq!(text, pin_radius_six_tenths());
    }

    #[test]
    fn a_saves_answer_behind_another_clients_begin_and_edit_shows_no_banner() {
        // PinA (2), saved (3), the transaction ended; then another client of the session
        // begins one and renames the pin in it (4), read from `status` before the save's
        // answer is taken. The answer, at 3, is behind the tab, but the session's move is
        // inside a transaction, so no reload: no banner.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        assert!(sessions.begin(&tab.id).wait().result.is_ok());
        let renamed = sessions
            .set_field(
                &tab.id,
                rev(3),
                pin,
                "name".to_owned(),
                silico_core::document::json::read(br#""Nut""#).unwrap(),
            )
            .wait();
        assert_eq!(renamed.session.unwrap().revision, rev(4));
        status_now(&mut tab, &sessions);
        taken_until(&mut tab, &sessions, Tab::quiet);
        assert!(tab.transaction);
        assert_eq!(banner(&tab), None);
    }

    #[test]
    fn a_reload_opens_sketch_mode_and_a_new_features_dialog_at_the_reloads_revision() {
        // The tab still at 3 as a reload to 5 is seen (a begin's answer taken before any
        // status read it): each opens at 5, no transaction open there, so what's sent after is
        // its own.
        let mut sketching = tab();
        sketching.revision = rev(3);
        sketching.sketch = Some(SketchMode::new(
            1,
            Some(FeatureId::new("s0000001").unwrap()),
        ));
        sketching.sketch_edited = true;
        sketching.reloaded_under_dialog(rev(5));
        // Opened again at the reload: nothing of the tab's applied since, as at any opening.
        assert!(!sketching.sketch_edited);
        assert_eq!(sketching.sketch.as_ref().unwrap().opened_at, Some(rev(5)));
        assert_eq!(
            (sketching.dialog_base, sketching.dialog_began),
            (Some(rev(5)), true)
        );
        let mut padding = tab();
        padding.revision = rev(3);
        padding.dialog = Some(Dialog::new(Kind::Pad, None, 1));
        padding.reloaded_under_dialog(rev(5));
        assert!(padding.dialog.is_some());
        assert_eq!(
            (padding.dialog_base, padding.dialog_began),
            (Some(rev(5)), true)
        );
    }

    #[test]
    fn a_new_features_dialog_open_through_a_save_answered_after_a_reload_opens_at_the_reload() {
        // PinA listed; PinB answered; a new Pad's dialog opened, with nothing in it to add;
        // a save, applied (4); another program writes the file, and the session reloads it
        // (5), read from `status` before the save's answer is taken. The dialog, its feature
        // not added, stays open, opening at the reload's revision, as at a reload a poll sees.
        let (dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin, "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.open_dialog(&sessions, Kind::Pad, &crate::dialog::Remembered::default());
        taken_until(&mut tab, &sessions, Tab::quiet);
        assert!(
            tab.dialog
                .as_ref()
                .is_some_and(|dialog| dialog.feature.is_none())
        );
        assert_eq!(tab.dialog_base, Some(rev(3)));
        tab.save(&sessions, false);
        ended_off_poll(&tab, &sessions);
        pin_named_at(&dir.path().join("part.silico"), "Bolt");
        assert_eq!(reloaded_past(&tab, &sessions, rev(4)), rev(5));
        status_then_answers(&mut tab, &sessions);
        assert!(tab.dialog.is_some(), "{:?}", tab.message);
        assert_eq!(tab.dialog_base, Some(rev(5)));
        assert_eq!(banner(&tab), Some((rev(4), rev(5))));
    }

    #[test]
    fn a_save_and_close_answered_after_a_reload_with_a_dialog_open_still_closes() {
        // The pin's Edit Feature dialog open; Save and close asked (a save, or an untitled
        // part's Save As), and the pin's radius typed behind it; the save applied, another
        // program writes the file, and the session reloads it, read from `status` before the
        // save's answer is taken. The tab closes: nothing queued behind the save is sent, so
        // no transaction is open for the close to be refused in.
        for save_as in [false, true] {
            let (dir, sessions, mut tab, pin) = pin_renamed();
            tab.edit_feature(&sessions, &pin);
            polled_until(&mut tab, &sessions, |tab| {
                tab.dialog.is_some() && tab.quiet()
            });
            let file = if save_as {
                let copy = dir.path().join("copy.silico");
                tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, true);
                copy
            } else {
                tab.save(&sessions, true);
                dir.path().join("part.silico")
            };
            tab.edit(&sessions, field_set(&pin, "radius", "0.75"));
            ended_off_poll(&tab, &sessions);
            // 1 at open, PinA 2, the save 3; the reload 4.
            pin_named_at(&file, "Bolt");
            assert_eq!(reloaded_past(&tab, &sessions, rev(3)), rev(4));
            status_now(&mut tab, &sessions);
            taken_until(&mut tab, &sessions, |tab| tab.closed || tab.close_refused);
            assert!(
                !tab.take_close_refused(),
                "save as: {save_as}: {:?}",
                tab.message
            );
            assert!(tab.closed, "save as: {save_as}");
        }
    }

    /// A fresh check of the tab's session, queued and running, every kernel request held (a
    /// hold's flag set first): what's queued on the session after it waits till it's let go.
    fn held_check(tab: &Tab, sessions: &FakeSessions) -> Running<CheckAnswer> {
        let check = sessions.check(&tab.id, CheckModel::Current { fresh: true });
        until("the check running", &|| {
            sessions.status(&tab.id).result.unwrap().running
                == Some(silico_core::command::RunningCommand::Command(
                    silico_core::command::SessionCommand::Check,
                ))
        });
        check
    }

    /// [`opened_on`] `text`, with `count` holds on its kernel, each holding every request
    /// that starts while its flag is set: each flag and its hold.
    fn opened_holding(
        text: &str,
        count: usize,
    ) -> (
        tempfile::TempDir,
        FakeSessions,
        Tab,
        Vec<(Arc<std::sync::atomic::AtomicBool>, silico_core::fake::Hold)>,
    ) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut holds = Vec::new();
        let (dir, sessions, tab) = opened_on(text, |kernel| {
            for _ in 0..count {
                let flag = Arc::new(AtomicBool::new(false));
                let holding = Arc::clone(&flag);
                holds.push((flag, kernel.hold(move |_| holding.load(Ordering::SeqCst))));
            }
        });
        (dir, sessions, tab, holds)
    }

    #[test]
    fn a_restores_put_back_refused_by_its_begins_reload_announces_that_reload_once() {
        use std::sync::atomic::Ordering;
        let (dir, sessions, mut tab, holds) = opened_holding(BLOCK, 2);
        let ((first, first_hold), (second, second_hold)) = (&holds[0], &holds[1]);
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let file = dir.path().join("part.silico");
        // Another program's write, reloaded (2) and announced by a poll.
        pin_named_at(&file, "Bolt");
        polled_until(&mut tab, &sessions, |tab| {
            banner(tab).is_some() && tab.quiet()
        });
        assert_eq!(banner(&tab), Some((rev(1), rev(2))));
        // Restore of 1: its read, of a revision, reads nothing of the file. The session's queue
        // then held behind a fresh check, so the put-back's begin and edit, sent at 2 once the
        // read answers, wait for it.
        tab.restore(&sessions, Revision::OPENED);
        first.store(true, Ordering::SeqCst);
        let check_first = held_check(&tab, &sessions);
        taken_until(&mut tab, &sessions, |tab| {
            tab.under
                .iter()
                .any(|under| matches!(under, Under::Replace { began: true, .. }))
        });
        // Another program's second write, before the begin runs: its read of the file reloads
        // it (3), so the put-back, sent against 2, is refused. Once a read behind them has
        // answered, a second check, held, so the reload's diff, asked as the first answer is
        // taken, can't answer before the second is.
        pin_named_at(&file, "Nut");
        let behind = sessions.text(&tab.id, None);
        first_hold.release();
        let behind = behind.wait();
        assert_eq!(behind.session.unwrap().revision, rev(3));
        second.store(true, Ordering::SeqCst);
        let check_second = held_check(&tab, &sessions);
        taken_until(&mut tab, &sessions, |tab| {
            !tab.under
                .iter()
                .any(|under| matches!(under, Under::Begin { .. } | Under::Replace { .. }))
        });
        // The reload announced once, from 2 to 3: no banner of its own listed as replaced.
        assert_eq!(banner(&tab), Some((rev(2), rev(3))));
        assert_eq!(banner_unlisted(&tab), NONE_UNLISTED);
        second_hold.release();
        let _ = (check_first.wait(), check_second.wait());
        // The transaction begun for the put-back rolled back: none open.
        polled_until(&mut tab, &sessions, Tab::quiet);
        assert!(!sessions.status(&tab.id).session.unwrap().transaction);
    }

    /// BLOCK open and listed (revision 1); then `change` asked of the tab and applied by the
    /// session, its revision read from `status` (the session's state now the one it made) but
    /// its answer not taken; and the opened revision's listing taken late, as a timeline read
    /// asked before it answers then. What Undo names once every answer is taken.
    fn named_after_a_late_listing(change: impl FnOnce(&mut Tab, &FakeSessions)) -> Option<String> {
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let listed = tab.timeline.clone().unwrap();
        assert_eq!(listed.revision, Revision::OPENED);
        change(&mut tab, &sessions);
        until("the change applied", &|| {
            revision_now(&tab, &sessions) != Revision::OPENED
        });
        status_now(&mut tab, &sessions);
        tab.took_timeline(listed);
        undo_named(&mut tab, &sessions)
    }

    #[test]
    fn a_timeline_taken_while_a_change_waits_keeps_the_names_of_the_state_it_was_sent_from() {
        // A suppression of the pin under way when the late listing is taken: the state it was sent
        // from keeps its names, and its step says "Suppress Pin".
        let named = named_after_a_late_listing(|tab, sessions| {
            tab.suppress_feature(sessions, FeatureId::new("a91cw0de").unwrap(), true);
        });
        assert_eq!(named.as_deref(), Some("Suppress Pin"));
    }

    #[test]
    fn a_timeline_taken_while_an_edit_is_in_flight_keeps_the_names_of_the_state_it_was_sent_from() {
        // Its twin for an edit in flight: the pin's radius edited, "Edit Pin".
        let named = named_after_a_late_listing(|tab, sessions| {
            tab.edit(
                sessions,
                Edit {
                    feature: FeatureId::new("a91cw0de").unwrap(),
                    field: "radius".to_owned(),
                    value: silico_core::document::json::read(b"0.75").unwrap(),
                },
            );
        });
        assert_eq!(named.as_deref(), Some("Edit Pin"));
    }

    #[test]
    fn a_timeline_taken_while_a_drags_end_waits_keeps_the_names_of_the_state_it_began_from() {
        // Its twin for a drag's end, kept and applied as `drive_drag` keeps and applies it (a
        // real drag needs sketch mode and a sketch worker): a drag of the Triangle sketch
        // begun at 1, its end's edit kept as sent from there; the session at 2 once its end
        // applied, read from `status`; 1's listing taken late; then the end's answer: "Edit
        // Triangle", named from the state the drag began from.
        let mut tab = tab();
        let triangle = FeatureId::new("s0000001").unwrap();
        tab.drag_sending = Some(tab.sending(Some(Words::Of("Edit", triangle))));
        stood_at(&mut tab, rev(2), 1, 0);
        tab.took_timeline(TimelineAnswer {
            revision: Revision::OPENED,
            ..named_timeline()
        });
        let sending = tab.drag_sending.take().unwrap();
        tab.applied(&sending, &answered_at(rev(2), Ok(())));
        assert_eq!(tab.undo_step().as_deref(), Some("Edit Triangle"));
    }

    #[test]
    fn what_waits_after_a_rename_decides_the_name_the_next_replaces() {
        // PinA, listed; PinB sent; then, queued behind it, a revert, a Reload (of the file as it
        // was opened), or a Restore of the revision the transaction began at (its read, then its
        // put-back, `replace_text`): each leaves the file's Pin, so the rename queued after it
        // replaces Pin.
        type Ask = fn(&mut Tab, &FakeSessions);
        let asks: [(&str, Ask); 3] = [
            ("revert", |tab, sessions| tab.revert(sessions, false)),
            ("reload", |tab, sessions| tab.reload(sessions)),
            ("restore", |tab, sessions| {
                tab.restore(sessions, Revision::OPENED)
            }),
        ];
        for (what, ask) in asks {
            let (_dir, sessions, mut tab, pin) = pin_renamed();
            tab.rename(&sessions, pin.clone(), "PinB");
            assert!(tab.edits.in_flight.is_some(), "{what}: PinB sent");
            ask(&mut tab, &sessions);
            tab.rename(&sessions, pin, "PinC");
            assert_eq!(
                undo_named(&mut tab, &sessions).as_deref(),
                Some("Rename Pin"),
                "{what}"
            );
        }
        // PinB answered and not yet listed, then an undo queued behind a rename in flight,
        // then a rename: the undo gives PinA back.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        taken_until(&mut tab, &sessions, Tab::quiet);
        tab.rename(&sessions, pin.clone(), "PinBB");
        tab.step(&sessions, true);
        tab.rename(&sessions, pin, "PinC");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename PinB")
        );
    }

    #[test]
    fn a_rename_in_flight_or_an_undo_under_way_or_answered_decides_the_name_the_next_replaces() {
        // A rename in flight gives the next its name; a rollback under way (a revert) gives the
        // file's back.
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.rename(&sessions, pin.clone(), "PinB");
        assert!(tab.edits.in_flight.is_some());
        tab.rename(&sessions, pin.clone(), "PinC");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename PinB")
        );
        let (_dir, sessions, mut tab, pin) = pin_renamed();
        tab.revert(&sessions, false);
        assert!(
            tab.under
                .iter()
                .any(|under| matches!(under, Under::Rollback { .. })),
            "the rollback under way"
        );
        tab.rename(&sessions, pin, "PinB");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename Pin")
        );
    }

    #[test]
    fn a_tool_edit_answered_names_no_rename() {
        // A boolean's `tool` set (a string, as a name is) is no rename: its answer says nothing of
        // a name, nor names the state it made. A name's is one.
        let set = |field: &str| FieldSet {
            feature: FeatureId::new("m3tq8v0z").unwrap(),
            field: field.to_owned(),
            before: Plain::String("a91cw0de".to_owned()),
            after: Plain::String("k7f2qx3m".to_owned()),
            changed: true,
            commit: None,
        };
        assert_eq!(renaming(&set("tool")), None);
        assert_eq!(
            renaming(&set("name")),
            Some((
                Words::Text("Rename a91cw0de".to_owned()),
                Effect::Names(FeatureId::new("m3tq8v0z").unwrap(), "k7f2qx3m".to_owned())
            ))
        );
    }

    #[test]
    fn a_poll_that_reads_the_new_revision_before_its_answer_is_taken_keeps_the_count() {
        // One rename counted. The session then moves past it (another client's rename here, as an
        // edit's own does once applied, before its answer is taken), with something of the tab's
        // under way (a `begin`): a poll reads a revision none of the tab's steps made, and keeps
        // the count it had.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        tab.rename(&sessions, FeatureId::new("a91cw0de").unwrap(), "PinA");
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.unsaved_edits() == 1
        });
        let moved = sessions
            .set_field(
                &tab.id,
                tab.revision,
                FeatureId::new("k7f2qx3m").unwrap(),
                "name".to_owned(),
                name_value("BlockB").unwrap(),
            )
            .wait();
        assert!(moved.result.is_ok());
        tab.beginning = true;
        tab.poll(&sessions);
        assert_eq!(tab.revision, moved.session.unwrap().revision);
        assert!(!tab.quiet());
        assert_eq!(tab.unsaved_edits(), 1);
    }

    #[test]
    fn a_rename_sent_as_a_saves_answer_is_taken_counts_none_of_the_saved_edits() {
        // PinA counted; Save, and a rename asked behind it. Once the save's answer is taken, PinB
        // is sent: the saved edit is no unsaved one, and PinB isn't applied yet, so none is
        // counted.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.rename(&sessions, pin.clone(), "PinA");
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.unsaved_edits() == 1
        });
        tab.save(&sessions, false);
        tab.rename(&sessions, pin, "PinB");
        taken_until(&mut tab, &sessions, |tab| {
            !tab.under
                .iter()
                .any(|under| matches!(under, Under::Commit { .. }))
                && tab.edits.in_flight.is_some()
        });
        assert_eq!(tab.unsaved_edits(), 0);
    }

    #[test]
    fn a_rename_after_one_answered_but_not_yet_read_again_replaces_the_name_that_one_gave() {
        // The pin renamed PinA, its answer taken (the session at revision 2) but the timeline not
        // read again (still revision 1's, "Pin"): the next rename replaces PinA, the name the
        // document holds when it applies.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.rename(&sessions, pin.clone(), "PinA");
        taken_until(&mut tab, &sessions, |tab| tab.edits.idle());
        assert!(
            tab.timeline
                .as_ref()
                .is_some_and(|timeline| timeline.revision < tab.revision),
            "the timeline read again"
        );
        assert_eq!(tab.name_of(&pin), "Pin");
        tab.rename(&sessions, pin, "PinAB");
        assert_eq!(
            undo_named(&mut tab, &sessions).as_deref(),
            Some("Rename PinA")
        );
    }

    #[test]
    fn the_unsaved_count_holds_while_an_edits_answer_is_yet_to_be_taken() {
        // One rename counted; a second sent, and `status` already of the revision it came to (poll
        // reads it before it takes the answers) with its answer not yet taken: the bar holds the
        // count it had, 1, not none for a frame.
        let (_dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        let pin = FeatureId::new("a91cw0de").unwrap();
        tab.rename(&sessions, pin.clone(), "PinA");
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && tab.unsaved_edits() == 1
        });
        tab.rename(&sessions, pin, "PinAB");
        assert!(tab.edits.in_flight.is_some(), "the second rename sent");
        let before = tab.revision;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let moved = loop {
            let at = revision_now(&tab, &sessions);
            if at != before {
                break at;
            }
            assert!(std::time::Instant::now() < deadline, "never applied");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        // As poll's `status` leaves it, the answer not yet taken.
        tab.revision = moved;
        tab.status_revision = moved;
        assert_eq!(tab.unsaved_edits(), 1);
        // Taken: two.
        polled_until(&mut tab, &sessions, |tab| tab.quiet());
        assert_eq!(tab.unsaved_edits(), 2);
    }

    #[test]
    fn a_save_as_ends_the_steps_with_the_transaction_so_the_next_edit_counts_alone() {
        // The pin renamed, then saved as another file: the transaction, and its undo stack,
        // end. The block renamed after: one unsaved edit, the only step Undo names, Redo
        // none (by hand).
        let (dir, sessions, mut tab) = opened_on(BLOCK, |_| {});
        polled_until(&mut tab, &sessions, |tab| tab.timeline.is_some());
        tab.rename(&sessions, FeatureId::new("a91cw0de").unwrap(), "PinA");
        polled_until(&mut tab, &sessions, |tab| tab.quiet() && tab.undoable());
        assert_eq!(tab.unsaved_edits(), 1);
        let copy = dir.path().join("copy.silico");
        tab.save_as(&sessions, copy.to_str().unwrap().to_owned(), false, false);
        polled_until(&mut tab, &sessions, |tab| {
            tab.quiet() && !tab.transaction && copy.exists()
        });
        assert_eq!(tab.unsaved_edits(), 0);
        tab.rename(&sessions, FeatureId::new("k7f2qx3m").unwrap(), "BlockB");
        polled_until(&mut tab, &sessions, |tab| tab.quiet() && tab.undoable());
        assert_eq!(tab.unsaved_edits(), 1);
        assert_eq!(tab.undo_step().as_deref(), Some("Rename Block"));
        assert_eq!(tab.redo_step().as_deref(), None);
    }

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
            consumers: Default::default(),
            suppressed: false,
        };
        let mut tab = tab();
        tab.browser = Browser::new(
            Vec::new(),
            vec![row("s0000001", "Loose"), row("s0000002", "Tight")],
        );
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
