//! One open document (docs/design/m5.0-app.md, section 2, *What a tab holds*): its session,
//! the newest answers of the commands it reads each frame or on a change, the commands it
//! has under way, and its edits, at most one in flight (section 3, *Bursts of edits*).
//! Every command is started here and polled with `try_take`; nothing here waits.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::command::{
    Added, Begun, Cancellable, CheckAnswer, CheckModel, Closed, Committed, FieldSet, ModelChoice,
    Problems, RebuildState, Response, Revision, RolledBack, Running, SessionId, Sessions, Status,
    Stepped, TextAnswer, TimelineAnswer,
};
use silico_core::disk::Disk;
use silico_core::document::FeatureId;
use silico_core::document::json::Node;
use silico_core::kernel::Clients;
use silico_core::timeline::{Entry, Health};

use crate::dialog::{Dialog, Kind, feature_node};

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
    /// Save As (`commit` with a path): to `path`, replacing a file there if `replace`.
    SaveAs {
        path: String,
        replace: bool,
    },
    Revert {
        then_close: bool,
    },
    Step {
        undo: bool,
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
    /// A dialog opened: the revision it opens at is the one everything asked before it
    /// comes to, which its Cancel undoes back to and its OK groups from.
    Opened,
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
    Closed {
        ok: bool,
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
    Undo(Option<Revision>),
    Redo(Option<Revision>),
    Commit,
    Rollback,
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
                | Queued::Add { .. }
                | Queued::Opened
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
    Step(Running<Stepped>),
    /// A Save As, the path it was given, which the tab asks to replace if a file is
    /// there, and whether the tab began the transaction it commits (rolled back if it's
    /// refused, so no empty transaction stays open).
    SaveAs {
        running: Running<Committed>,
        path: String,
        began: bool,
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

/// The browser's rows, worked out once for each timeline read, never each frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Browser {
    pub bodies: Vec<Row>,
    pub sketches: Vec<Row>,
}

/// One open document.
pub struct Tab {
    pub id: SessionId,
    /// The path, as it was opened.
    pub path: String,
    /// The session's revision and whether a transaction is open, as last answered: never
    /// moved back by an answer that comes after a newer one.
    pub revision: Revision,
    pub transaction: bool,
    /// The newest `status`, and the revision it was of.
    pub status: Option<Status>,
    status_revision: Revision,
    /// The revision of the newest model published, if there's one.
    pub published: Option<Revision>,
    /// The newest `timeline`, and the browser's rows worked out from it.
    pub timeline: Option<TimelineAnswer>,
    pub browser: Browser,
    timeline_read: Option<Running<TimelineAnswer>>,
    timeline_asked: Option<(Revision, Option<Revision>)>,
    /// A published timeline was refused, nothing being published: the next is read of the
    /// current model, which waits for its rebuild (off the UI thread).
    nothing_published: bool,
    /// What `check` says of each feature (read when the person opens a mark, section 3),
    /// and of the document (the gate's problems).
    pub said: BTreeMap<String, Said>,
    pub document_problems: Vec<Said>,
    /// A `check` read, and whether it's of a document the gate refused (whose problems are
    /// all the document's).
    check_read: Option<(bool, Running<CheckAnswer>)>,
    checked: Option<(Revision, Option<Revision>)>,
    pub edits: Edits,
    under: Vec<Under>,
    /// How many saves it has sent: what a test counts.
    pub saves: u64,
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
    /// The feature selected in the timeline or the browser.
    pub selected: Option<FeatureId>,
    /// The browser's hidden rows, by their names: the tab's state, never saved (open item
    /// 29).
    pub hidden: std::collections::BTreeSet<String>,
    /// The last refusal a command of the tab's answered, as said to the person, until
    /// their next action (an edit, a save, a revert, a step).
    pub message: Option<String>,
    /// A save or a revert that was to close the tab was refused: the tab stays, and a quit
    /// waiting on it stops ([`take_close_refused`](Self::take_close_refused)).
    close_refused: bool,
    /// A Save As found a file at the path it was given, and wasn't to replace it: the path,
    /// for the window to ask whether to ([`take_replace_asked`](Self::take_replace_asked)).
    replace_asked: Option<String>,
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
    /// The dialogs' edits OK'd in this transaction, each one step for undo and redo.
    groups: Vec<Group>,
    /// Each revision an undo or a redo came to, and the first revision of the state it
    /// restored: what names a state, whichever revision the session is at it by.
    restored: BTreeMap<Revision, Revision>,
    /// The fields whose last edit was refused, by feature, since the dialog opened: its
    /// input of one is red, and its OK off, until an edit of it is taken.
    refused: std::collections::BTreeSet<(FeatureId, String)>,
    /// The commands it sent that change its transaction, in order, the newest
    /// [`SENT_KEPT`]: the call record.
    pub sent: Vec<Sent>,
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
        Tab {
            id,
            path,
            revision: Revision::OPENED,
            transaction: false,
            status: None,
            status_revision: Revision::OPENED,
            published: None,
            timeline: None,
            browser: Browser::default(),
            timeline_read: None,
            timeline_asked: None,
            nothing_published: false,
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
            checked: None,
            edits: Edits::default(),
            under: Vec::new(),
            saves: 0,
            resets: 0,
            beginning: false,
            disk_choice: DiskChoice::None,
            selected: None,
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
            groups: Vec::new(),
            restored: BTreeMap::new(),
            refused: std::collections::BTreeSet::new(),
            sent: Vec::new(),
        }
    }

    /// The tab's name: its file's.
    pub fn name(&self) -> &str {
        std::path::Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
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
                .any(|under| matches!(under, Under::Step(_)))
    }

    /// Whether Undo can step: an edit still to answer (the undo waits behind it), or an
    /// edit to undo as the status of the tab's newest revision counts them; never while
    /// another step is under way, whose answer moves the counts, nor while a dialog is open,
    /// whose Cancel undoes its edits.
    pub fn undoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
            && !self.stepping()
            && (!self.edits.idle()
                || (self.synced()
                    && self
                        .transaction()
                        .is_some_and(|transaction| transaction.undo > 0)))
    }

    /// Whether Redo can step: an edit undone, and none made since, as the status of the
    /// tab's newest revision counts them; never while a step is under way, nor while a
    /// dialog is open.
    pub fn redoable(&self) -> bool {
        self.transaction
            && !self.in_dialog()
            && self.edits.idle()
            && !self.stepping()
            && self.synced()
            && self
                .transaction()
                .is_some_and(|transaction| transaction.redo > 0)
    }

    /// Whether it holds unsaved edits: the document changed, an edit under way, or a
    /// transaction open with anything of the tab's still to answer (an undo or a redo
    /// queued or running among them). Only a tab with
    /// none of these is closed without asking (rolled back first if a transaction is open).
    pub fn unsaved(&self) -> bool {
        // A `begin` under way goes with the edit it was sent for (`edits.idle` is false).
        self.changed() || !self.edits.idle() || (self.transaction && !self.quiet())
    }

    /// Save As (`commit` with a path, docs/design/m5.0-app.md §4): once everything asked
    /// before it has answered, as a save, the document written to `path` (replacing a file
    /// there if `replace`), the tab then of the new file. A file there and no `replace` is
    /// asked about ([`take_replace_asked`](Self::take_replace_asked)); any other refusal is
    /// said.
    pub fn save_as<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        path: String,
        replace: bool,
    ) {
        self.message = None;
        self.edits.queue.push_back(Queued::SaveAs { path, replace });
        self.advance(sessions);
    }

    /// The path a Save As found a file at, if one did since this was last asked: whether to
    /// replace it is the person's to say.
    pub fn take_replace_asked(&mut self) -> Option<String> {
        self.replace_asked.take()
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
            // Moved with nothing of the tab's under way: the session's own reload.
            if self.status.is_some() && session.revision != self.revision && self.quiet() {
                self.resets += 1;
                reloaded = true;
            }
            self.revision = session.revision;
            self.transaction = session.transaction;
            self.status_revision = session.revision;
        }
        match status.result {
            Ok(status) => self.status = Some(status),
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
        self.published = published.result.ok().map(|published| published.revision);
        if self.published.is_some() {
            self.nothing_published = false;
        }
        self.take_answers(sessions);
        self.offer_disk_choice();
        self.read_timeline(sessions);
        self.read_check(sessions);
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
        if let Some(read) = self.timeline_read.take() {
            match read.try_take() {
                Ok(response) => match response.result {
                    Ok(timeline) => self.took_timeline(timeline),
                    Err(problems) => {
                        // Nothing published yet: read the current model's, which waits for
                        // its rebuild, next.
                        self.nothing_published = problems
                            .errors()
                            .iter()
                            .any(|error| error.parameter() == Some("model"));
                        self.timeline_asked = None;
                    }
                },
                Err(read) => self.timeline_read = Some(read),
            }
        }
        if let Some((gate, read)) = self.check_read.take() {
            match read.try_take() {
                Ok(response) => {
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
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    match response.result {
                        // The session holds the new file from now on, by the path given.
                        Ok(committed) => {
                            self.path = committed.path;
                            self.disk_choice = DiskChoice::None;
                        }
                        Err(problems) => {
                            // Its own transaction, begun for it, ended: it held nothing.
                            self.roll_back |= began;
                            if problems.errors().first().and_then(|e| e.suggestion())
                                == Some(silico_core::file::REPLACE_ADVICE)
                            {
                                self.replace_asked = Some(path);
                            } else {
                                self.message = Some(said(&problems));
                            }
                        }
                    }
                    None
                }
                Err(running) => Some(Under::SaveAs {
                    running,
                    path,
                    began,
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
            Under::Step(running) => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    if let (Ok(stepped), Some(session)) = (&response.result, &response.session) {
                        let state = self.state_of(stepped.restored);
                        self.restored.insert(session.revision, state);
                    }
                    match response.result {
                        // Typing queued after it is applied next: the field keeps it.
                        Ok(_) if self.edits.idle() => self.resets += 1,
                        Ok(_) => {}
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Step(running)),
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
                                self.selected = Some(added.feature.id);
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
                Queued::Opened => {
                    self.dialog_base = Some(self.revision);
                    self.dialog_began = !self.transaction && !self.beginning;
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
                Queued::Closed { ok } => {
                    let Some(base) = self.dialog_base.take() else {
                        continue;
                    };
                    if !self.transaction {
                        // Nothing of the dialog's changed the document.
                        continue;
                    }
                    if !ok && self.dialog_began {
                        // Its own edits began the transaction: nothing before it to keep, and
                        // no transaction left open with nothing in it (an outside change is
                        // then reloaded, section 4).
                        self.supersede_preview();
                        self.record(Sent::Rollback);
                        self.under.push(Under::Rollback {
                            running: sessions.rollback(&self.id),
                            then_close: false,
                        });
                    } else if base == self.revision {
                        // Nothing of the dialog's changed the document.
                    } else if ok {
                        self.groups.push(Group {
                            start: self.state_of(base),
                            end: self.state_of(self.revision),
                        });
                    } else {
                        self.supersede_preview();
                        self.record(Sent::Undo(Some(base)));
                        self.under.push(Under::Step(sessions.undo(
                            &self.id,
                            self.revision,
                            Some(base),
                        )));
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
                Queued::SaveAs { path, replace } => {
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
                Queued::Step { undo } => {
                    // At a dialog's OK'd edits' end (undo) or start (redo): all of them at
                    // once, to its other side (section 9).
                    let state = self.state_of(self.revision);
                    let to = self.groups.iter().find_map(|group| match undo {
                        true => (group.end == state).then_some(group.start),
                        false => (group.start == state).then_some(group.end),
                    });
                    self.supersede_preview();
                    let running = if undo {
                        self.record(Sent::Undo(to));
                        sessions.undo(&self.id, self.revision, to)
                    } else {
                        self.record(Sent::Redo(to));
                        sessions.redo(&self.id, self.revision, to)
                    };
                    self.under.push(Under::Step(running));
                }
            }
        }
    }

    /// Whether what's queued waits: an edit, a save, a revert, a step, or a dialog's add (or
    /// its body's read) running.
    fn blocked(&self) -> bool {
        self.edits.in_flight.is_some()
            || self.under.iter().any(|under| {
                matches!(
                    under,
                    Under::Commit { .. }
                        | Under::SaveAs { .. }
                        | Under::Rollback { .. }
                        | Under::Step(_)
                        | Under::Add { .. }
                        | Under::Base { .. }
                )
            })
    }

    /// Whether nothing of the tab's is under way: no edit in flight or queued, nothing
    /// queued, no command running. A revision that moves then isn't the tab's.
    fn quiet(&self) -> bool {
        self.edits.in_flight.is_none()
            && self.edits.queue.is_empty()
            && self.under.is_empty()
            && !self.beginning
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

    /// Whether it's closing: a save or a revert that closes it, or its close, queued or
    /// running. The property panel takes no edit then: it would be dropped.
    pub fn closing(&self) -> bool {
        self.edits.queue.iter().any(|queued| {
            matches!(
                queued,
                Queued::Save { then_close: true } | Queued::Revert { then_close: true }
            )
        }) || self.under.iter().any(|under| {
            matches!(
                under,
                Under::Commit {
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
        // The document goes back to the file's: a dialog open on it has nothing to edit.
        self.dialog = None;
        self.loading = None;
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
        self.edits.queue.push_back(Queued::Step { undo });
        self.advance(sessions);
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
    /// if one is (that one answers `cancelled`). One that comes after the rebuild ended is
    /// refused, and the refusal is said.
    pub fn stop<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        match sessions.cancel(&self.id, Cancellable::Rebuild).result {
            Ok(_) => self.message = None,
            Err(problems) => self.message = Some(said(&problems)),
        }
    }

    /// A dialog's preview stopped, with the stop's call: refused only if the preview has
    /// finished since, which leaves nothing to stop and nothing to say.
    pub fn stop_preview<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if sessions
            .cancel(&self.id, Cancellable::Rebuild)
            .result
            .is_ok()
        {
            self.message = None;
        }
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

    /// Whether the rebuild of the tab's revision was stopped, as `status` says.
    fn rebuild_stopped(&self) -> bool {
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

    /// Before the tab sends what changes the document: its timeline read in flight, of a
    /// revision this makes old, cancelled through its handle (queued or running), so what's
    /// sent doesn't wait for a rebuild nothing will show (section 3, *Bursts of edits*): a
    /// preview's, which waits for the rebuild (a read of the published model answers at
    /// once, so there's nothing of it to stop).
    fn supersede_preview(&mut self) {
        if let Some(read) = &self.timeline_read {
            read.cancel();
        }
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

    /// Whether a dialog of `kind` can open: none open or loading, the tab not closing, the
    /// gate taking the document, and, for one that cuts, a body to cut from.
    pub fn can_open(&self, kind: Kind) -> bool {
        self.dialog.is_none()
            && self.loading.is_none()
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
        let sketch = self.selected.clone().filter(|id| self.is_sketch(id));
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
        self.edits.queue.push_back(Queued::Opened);
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
        self.selected = Some(id.clone());
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
        self.edits.queue.push_back(Queued::Closed { ok });
        self.advance(sessions);
    }

    /// Whether a dialog is open, or its feature is being read for one.
    pub fn in_dialog(&self) -> bool {
        self.dialog.is_some() || self.loading.is_some()
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
        let Some((gate, model)) = check_wanted(timeline, self.selected.as_ref()) else {
            return;
        };
        self.checked = Some(asked);
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
    fn an_answer_moves_the_tabs_revision_and_transaction_forward_never_back() {
        let info = |revision: u64, transaction: bool| silico_core::command::SessionInfo {
            id: SessionId::parse("s1").unwrap(),
            path: "a.silico".to_owned(),
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
        tab.selected = Some(FeatureId::new("a0000001").unwrap());
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
}
