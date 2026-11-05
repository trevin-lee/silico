//! One open document (docs/design/m5.0-app.md, section 2, *What a tab holds*): its session,
//! the newest answers of the commands it reads each frame or on a change, the commands it
//! has under way, and its edits, at most one in flight (section 3, *Bursts of edits*).
//! Every command is started here and polled with `try_take`; nothing here waits.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::export::{Export, Exported};
use silico_core::command::{
    Added, Begun, Cancellable, CheckAnswer, CheckModel, Closed, Committed, ExtendBy, FieldSet,
    ModelChoice, Moved, NewMembers, Problems, RebuildState, Response, Revision, RolledBack,
    Running, SessionId, Sessions, SketchCut, SketchEdited, SketchJoined, Status, Stepped,
    TextAnswer, TimelineAnswer,
};
use silico_core::disk::Disk;
use silico_core::document::json::Node;
use silico_core::document::{ElementId, FeatureId, Reference};
use silico_core::extend::End;
use silico_core::kernel::Clients;
use silico_core::sketch_tools::{AddToken, OfferChoice, OfferRequest};
use silico_core::timeline::{Entry, Health};

use crate::dialog::{Dialog, Kind, feature_node, quoted};
use crate::sketch::dimension::{Ask, ModifyAction};
use crate::sketch::drag::{Drag, News};
use crate::sketch::tools::{Choice, Request};
use crate::sketch::{Deletion, FrameInput, Prefixes, SketchMode};

/// What's said when an over-defining dimension's choice is refused as a `conflict`: the
/// session moved on before it was sent, so it was overtaken, and isn't sent again (SD7: its
/// requests are made against the revision its add or edit made).
pub const OVERTAKEN: &str = "The dimension's choice wasn't applied: the sketch changed after it was placed (another edit came first), so it isn't sent again";

/// What a feature's mark says: its first problem's (or warning's) message, and its
/// suggestion, from a `check` of the model the timeline's marks came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub message: String,
    pub suggestion: Option<String>,
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

/// A command a tab sent that changes its transaction, as it sent it: what a test reads
/// as the call record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    Begin,
    Add,
    Edit(String),
    /// `move_feature`, of the feature, before the feature (none: to the end).
    Move(FeatureId, Option<FeatureId>),
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

/// A tab's edits and what waits with them: at most one edit in flight, the rest queued
/// ([`Queued`]).
#[derive(Default)]
pub struct Edits {
    /// The edit in flight: its feature and field, as it was sent, and its answer to come.
    in_flight: Option<((FeatureId, String), Sending, Running<FieldSet>)>,
    queue: std::collections::VecDeque<Queued>,
    /// How many edits it has sent: what a test counts.
    pub sent: u64,
}

impl Edits {
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
                | Queued::Step { .. }
                | Queued::Move { .. }
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
                | Queued::Closed { .. } => break,
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
/// file read gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reloaded {
    pub from: Revision,
    pub to: Revision,
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
    /// What's selected (M5.7): features of the timeline or the browser, faces and edges of
    /// the canvas, by name; and what the pointer is over.
    selection: crate::selection::Selection,
    /// The canvas's box selection (M5.7): a left drag, on geometry or empty space.
    box_select: crate::box_select::BoxSelect,
    /// What a toast is to say for the tab (a box's count, or why it took nothing), until
    /// the app takes it.
    toast: Option<String>,
    /// What the browser's eyes hide: the tab's state, never saved (open item 29).
    pub hidden: std::collections::BTreeSet<Hideable>,
    /// The last refusal a command of the tab's answered, as said to the person, until
    /// their next action (an edit, a save, a revert, a step).
    pub message: Option<String>,
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
    /// The dialogs' edits OK'd in this transaction, each one step for undo and redo.
    groups: Vec<Group>,
    /// The state sketch mode's Finish ended at: Redo is off there till the next edit, so
    /// what it undid isn't redone past its Finish.
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
            document_problems: problems
                .errors()
                .iter()
                .map(|error| Said {
                    message: error.message().to_owned(),
                    suggestion: error.suggestion().map(str::to_owned),
                })
                .collect(),
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
            selection: crate::selection::Selection::default(),
            box_select: crate::box_select::BoxSelect::default(),
            toast: None,
            hidden: std::collections::BTreeSet::new(),
            message: None,
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
            view: crate::viewport::View::default(),
            drag: None,
            drag_sending: None,
            watch: None,
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
    /// With no sketch open, the left button and the pointer are the selection's (M5.7):
    /// a click picks what's under it, the hover what the pointer is over, a drag on
    /// geometry what's under its press, and a drag from empty space a box
    /// (`crate::box_select`), each read against the published model the tab last polled,
    /// which asks the session nothing more. The box select comes after the selection, so
    /// it reads the hover this frame's answer gave.
    /// SEAM(#351): sketch mode claims them first, while a sketch is open
    /// (`if self.in_sketch() { self.selection.canvas_claimed(); self.box_select =
    /// Default::default(); return self.sketch.overlay(sessions, painter, frame) }`, a click
    /// of the selection's waiting and a box drawn let go), its face pick for Create Sketch
    /// reading [`selection`](Self::selection)'s hover and pick.
    pub fn overlay<P: Clients + 'static>(
        &mut self,
        _sessions: &Arc<Sessions<P>>,
        painter: &eframe::egui::Painter,
        frame: &crate::viewport::ViewFrame,
    ) {
        self.selection.overlay(&mut self.view, frame, painter.ctx());
        // The bodies are of the model the selection is read against: the one published
        // last, as counted.
        let current = matches!(
            (self.published, self.view.bodies_of()),
            (Some(revision), Some(of)) if of == (self.published_models, revision)
        );
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
    }

    /// What a toast is to say for the tab, taken once (the app shows it).
    pub fn take_toast(&mut self) -> Option<String> {
        self.toast.take()
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
    /// whose Cancel undoes its edits. In sketch mode, as SolidWorks allows, but never past
    /// the state it opened at.
    pub fn undoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
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
    /// dialog is open. In sketch mode, only what it undid itself.
    pub fn redoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
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

    /// Whether a move, a dialog's add, or sketch mode's add, edit, dimension's value, or
    /// over-defining dimension's choice is queued or running: each changes the document,
    /// and may be under way before the transaction its `begin` opens is.
    fn changing(&self) -> bool {
        self.edits.queue.iter().any(|queued| {
            matches!(
                queued,
                Queued::Move { .. }
                    | Queued::Add { .. }
                    | Queued::AddSketch { .. }
                    | Queued::SketchRemove { .. }
                    | Queued::SketchJoin { .. }
                    | Queued::SketchAdd { .. }
                    | Queued::SketchTrim { .. }
                    | Queued::SketchSplit { .. }
                    | Queued::SketchExtend { .. }
                    | Queued::DimensionValue { .. }
                    | Queued::Offer { .. }
            )
        }) || self.under.iter().any(|under| {
            matches!(
                under,
                Under::Move { .. }
                    | Under::Add { .. }
                    | Under::Base { .. }
                    | Under::AddSketch { .. }
                    | Under::SketchEdit { .. }
                    | Under::SketchJoin { .. }
                    | Under::SketchAdd { .. }
                    | Under::SketchCut { .. }
                    | Under::DimensionValue { .. }
                    | Under::OfferField { .. }
            )
        })
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
        self.message = None;
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
                self.resets += 1;
                reloaded = true;
                self.reloaded = if std::mem::take(&mut self.reload_asked) {
                    None
                } else {
                    Some(Reloaded {
                        from: self.revision,
                        to: session.revision,
                    })
                };
            }
            self.revision = session.revision;
            self.transaction = session.transaction;
            self.status_revision = session.revision;
        }
        match status.result {
            Ok(status) => {
                // The file as the session holds it, nothing of the tab's under way: no
                // reload of the person's is still to come.
                if self.quiet() && !matches!(status.disk, Some(Disk::Changed | Disk::Gone)) {
                    self.reload_asked = false;
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
                Ok(response) => {
                    self.check_superseded = false;
                    if std::mem::take(&mut self.fresh_out) {
                        // Rebuild everything's: what it published is read again.
                        self.timeline_asked = None;
                    }
                    if let Ok(answer) = response.result {
                        self.took_check(&answer, gate);
                    }
                }
                Err(read) => self.check_read = Some((gate, read)),
            }
        }
        let mut close = false;
        for under in std::mem::take(&mut self.under) {
            if let Some(under) = self.take(under, &mut close) {
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
        }
        if timeline.problems == 0 {
            self.document_problems.clear();
        }
        self.browser = browser(&timeline);
        self.timeline = Some(timeline);
    }

    /// `under` polled: none if it answered (what it changed taken), or it back.
    fn take(&mut self, under: Under, close: &mut bool) -> Option<Under> {
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
                                dialog.adding = false;
                                dialog.feature = Some(added.feature.id.clone());
                                // Selected, so its mark's text is read and the property
                                // panel shows it.
                                self.selection.select_feature(added.feature.id);
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
                            Ok(Some(dialog)) => self.dialog = Some(dialog),
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
        if let Some((field, sending, running)) = self.edits.in_flight.take() {
            match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    self.applied(sending, &response);
                    // Said, the field keeping what the person typed (#298's review): a
                    // conflict's too, whose next edit is sent against the reloaded revision.
                    match response.result {
                        Ok(_) => {
                            self.refused.remove(&field);
                        }
                        Err(problems) => {
                            self.message = Some(said(&problems));
                            self.refused.insert(field);
                        }
                    }
                }
                Err(running) => {
                    self.edits.in_flight = Some((field, sending, running));
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
                    let sending = self.sending();
                    let running = sessions.set_field(
                        &self.id,
                        self.revision,
                        edit.feature,
                        edit.field,
                        edit.value,
                    );
                    self.edits.in_flight = Some((field, sending, running));
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
                Queued::Closed { ok, sketch } => {
                    let Some(base) = self.dialog_base.take() else {
                        continue;
                    };
                    if !self.transaction {
                        // Nothing of the dialog's changed the document.
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
                    } else if ok {
                        self.groups.push(Group {
                            start: self.state_of(base),
                            end: self.state_of(self.revision),
                        });
                    } else {
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

    /// Whether what's queued waits: an edit, a save, a revert, a step, or a dialog's add (or
    /// its body's read) running.
    fn blocked(&self) -> bool {
        self.edits.in_flight.is_some()
            || self.dragging()
            || self.under.iter().any(|under| {
                matches!(
                    under,
                    Under::Commit { .. }
                        | Under::SaveAs { .. }
                        | Under::Rollback { .. }
                        | Under::Step { .. }
                        | Under::Move { .. }
                        | Under::Add { .. }
                        | Under::Base { .. }
                        | Under::AddSketch { .. }
                        | Under::SketchEdit { .. }
                        | Under::SketchJoin { .. }
                        | Under::SketchAdd { .. }
                        | Under::SketchCut { .. }
                        | Under::DimensionValue { .. }
                        | Under::OfferField { .. }
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
        self.message = None;
        self.edits.push(edit);
        self.advance(sessions);
    }

    /// `feature` moved along the timeline before `before`, or to the end for none
    /// (`move_feature`), as an edit is: inside the tab's transaction, begun first if none is
    /// open, and sent once everything asked before it has answered. Never while a dialog is
    /// open (its Cancel undoes to where it opened) or the tab closes: then it's said why, and
    /// nothing is sent. A refusal is said in the status bar.
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
        if self.closing() {
            return;
        }
        self.message = None;
        self.edits.queue.push_back(Queued::Move { feature, before });
        self.advance(sessions);
    }

    /// Save (`commit`), then, if `then_close`, close: once everything asked before it has
    /// answered, so the commit saves every edit made before it. One queued last already takes
    /// `then_close` too. A save that comes up with nothing to save does nothing (or closes).
    pub fn save<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, then_close: bool) {
        self.message = None;
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
        self.message = None;
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
        self.message = None;
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
        self.message = None;
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

    /// Esc with no dialog open (section 3's order, the nearest thing first): sketch mode's
    /// tool left, its chain or gesture ended with it (section 9, *Line*); or the selection
    /// cleared (sketch mode's first, then the feature's); or, with none, a rebuild running
    /// stopped quietly ([`stop_quietly`](Self::stop_quietly)); with neither, the reload's
    /// banner dismissed, if one shows.
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
        } else if !self.selection.is_empty() {
            self.selection.clear();
        } else if self.stoppable() {
            self.stop_quietly(sessions);
        } else {
            // Last, the purple banner dismissed (DESIGN.md, 6c; an amber or red one in its
            // place has let it go already).
            self.reloaded = None;
        }
    }

    /// The stop, from Esc or a dialog preview: as the button's ([`stop`](Self::stop)), but
    /// a refusal (the rebuild finished since `status` said it ran, leaving nothing to stop)
    /// says nothing. Only the Stop button says its refusal. Whether it stopped the rebuild.
    pub fn stop_quietly<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) -> bool {
        let stopped = self.halt(sessions).is_ok();
        if stopped {
            self.message = None;
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
        let disk = self.status.as_ref().and_then(|status| status.disk);
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
    /// closing, the
    /// gate taking the document, and, for one that cuts, a body to cut from.
    pub fn can_open(&self, kind: Kind) -> bool {
        self.dialog.is_none()
            && self.loading.is_none()
            && self.sketch.is_none()
            && !self.closing()
            && self
                .timeline
                .as_ref()
                .is_some_and(|timeline| timeline.problems == 0)
            && (!kind.cuts() || self.tip().is_some())
    }

    /// A new feature's dialog opened (section 9): its inputs at their defaults, a sweep's
    /// sketch the one selected if it's one, and its body the tip. Its feature is added as
    /// soon as its inputs are valid.
    pub fn open_dialog<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, kind: Kind) {
        if !self.can_open(kind) {
            return;
        }
        let serial = self.opening();
        let sketch = self
            .selection
            .feature()
            .cloned()
            .filter(|id| self.is_sketch(id));
        self.dialog = Some(Dialog::new(kind, sketch, serial));
        self.dialog_changed(sessions);
        self.advance(sessions);
    }

    /// A dialog opening: numbered, the revision it opens at marked in the queue, and what
    /// the last one left (a refusal said, its refused fields) let go. Its number.
    fn opening(&mut self) -> u64 {
        self.message = None;
        self.refused.clear();
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
        let Some(dialog) = self.dialog.as_mut() else {
            return false;
        };
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
        let changes = dialog.changes();
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
    pub fn close_dialog<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, ok: bool) {
        if self.closing() {
            return;
        }
        if self.dialog.take().is_none() && self.loading.take().is_none() {
            return;
        }
        self.edits
            .queue
            .push_back(Queued::Closed { ok, sketch: false });
        self.advance(sessions);
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
        self.message = None;
        self.edits.queue.push_back(match deletion {
            Deletion::Remove(members) => Queued::SketchRemove { sketch, members },
            Deletion::Join(point) => Queued::SketchJoin { sketch, point },
        });
        self.advance(sessions);
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
    /// preview's check gives it, then the tab's last refusal.
    pub fn dialog_said(&self) -> Vec<String> {
        let feature = self
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.feature.as_ref());
        feature
            .and_then(|feature| self.said.get(feature.as_str()))
            .map(|said| said.message.clone())
            .into_iter()
            .chain(self.message.clone())
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
        let Some((gate, model)) = check_wanted(timeline, self.selection.feature()) else {
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
    /// refused (`gate`), or else those of no feature.
    fn took_check(&mut self, answer: &CheckAnswer, gate: bool) {
        let mut by_feature = BTreeMap::new();
        let mut document = Vec::new();
        for error in answer.problems.errors() {
            let said = Said {
                message: error.message().to_owned(),
                suggestion: error.suggestion().map(str::to_owned),
            };
            if gate || error.feature().is_none() {
                document.push(said.clone());
            }
            if let Some(feature) = error.feature() {
                by_feature.entry(feature.to_owned()).or_insert(said);
            }
        }
        for warning in answer.warnings.listed() {
            if let Some(feature) = warning.feature() {
                by_feature
                    .entry(feature.to_owned())
                    .or_insert_with(|| Said {
                        message: warning.message().to_owned(),
                        suggestion: warning.suggestion().map(str::to_owned),
                    });
            }
        }
        self.said = by_feature;
        self.document_problems = document;
    }
}

/// The body a new sweep builds on, as FreeCAD's PartDesign takes its body's tip, from a
/// document's features as `timeline` lists them, in order: the last that stands for a solid
/// and that no feature consumes. A feature stands for a solid if it makes one (it isn't a
/// sketch) and isn't suppressed, or if it's suppressed and its base stands for one (its
/// chain passes it through, as `document::suppression`'s `standing` has it). A feature is
/// consumed if any feature names it as an input, as the gate counts consumers: built or
/// not, suppressed or not (a sketch a sweep names is among them, and stands for nothing).
fn tip_of(entries: &[Entry]) -> Option<FeatureId> {
    let consumed: std::collections::BTreeSet<&FeatureId> = entries
        .iter()
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

/// Whether a `check` of `timeline` is wanted, with `selected` the feature selected: for a
/// document the gate refused (`gate`, true), of the current model, its problems all the
/// document's; or for a selected feature `timeline` marks, of the published model (the
/// current, while none is published). None for neither.
fn check_wanted(
    timeline: &TimelineAnswer,
    selected: Option<&FeatureId>,
) -> Option<(bool, CheckModel)> {
    let gate = timeline.problems > 0;
    let opened = selected.is_some_and(|selected| {
        timeline
            .features
            .iter()
            .flatten()
            .any(|entry| &entry.id == selected && (entry.problems > 0 || entry.warnings > 0))
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
            check_wanted(&timeline(0, Some(one), features()), None),
            None
        );
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&a)),
            None
        );
        // A selected feature with warnings, or problems: of the published model.
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&b)),
            Some((false, CheckModel::Published))
        );
        let c = FeatureId::new("c0000001").unwrap();
        assert_eq!(
            check_wanted(&timeline(0, Some(one), features()), Some(&c)),
            Some((false, CheckModel::Published))
        );
        // None published: of the current one.
        assert_eq!(
            check_wanted(&timeline(0, None, features()), Some(&b)),
            Some((false, CheckModel::Current { fresh: false }))
        );
        // A document the gate refused: the current model, whatever is selected.
        assert_eq!(
            check_wanted(&timeline(2, Some(one), features()), None),
            Some((true, CheckModel::Current { fresh: false }))
        );
        assert_eq!(
            check_wanted(&timeline(2, Some(one), features()), Some(&a)),
            Some((true, CheckModel::Current { fresh: false }))
        );
    }

    #[test]
    fn a_timeline_unmarked_drops_the_features_text_and_one_the_gate_took_the_documents() {
        let said = || Said {
            message: "m".to_owned(),
            suggestion: None,
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
}
