//! One open document (docs/design/m5.0-app.md, section 2, *What a tab holds*): its session,
//! the newest answers of the commands it reads each frame or on a change, the commands it
//! has under way, and its edits, at most one in flight (section 3, *Bursts of edits*).
//! Every command is started here and polled with `try_take`; nothing here waits.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::command::{
    Begun, CheckAnswer, CheckModel, Closed, Committed, FieldSet, ModelChoice, Problems, Response,
    Revision, RolledBack, Running, RunningCommand, SessionId, Sessions, Status, Stepped,
    TimelineAnswer,
};
use silico_core::disk::Disk;
use silico_core::document::FeatureId;
use silico_core::document::json::Node;
use silico_core::kernel::Clients;

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

/// A tab's edits (section 3): at most one in flight; those made meanwhile wait in the tab,
/// a newer edit of a field replacing the one waiting for it, each sent once the one before
/// answers, against the revision its answer gave.
#[derive(Default)]
pub struct Edits {
    in_flight: Option<Running<FieldSet>>,
    waiting: Vec<Edit>,
    /// How many it has sent: what a test counts.
    pub sent: u64,
}

impl Edits {
    /// Whether none is in flight or waiting.
    pub fn idle(&self) -> bool {
        self.in_flight.is_none() && self.waiting.is_empty()
    }

    /// `edit` made: waiting, in place of one of the same field that waits, or after them.
    fn push(&mut self, edit: Edit) {
        match self
            .waiting
            .iter_mut()
            .find(|waiting| waiting.feature == edit.feature && waiting.field == edit.field)
        {
            Some(waiting) => *waiting = edit,
            None => self.waiting.push(edit),
        }
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
    Close(Running<Closed>),
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

/// One open document.
pub struct Tab {
    pub id: SessionId,
    /// The path, as it was opened.
    pub path: String,
    /// The session's revision and whether a transaction is open, as last answered.
    pub revision: Revision,
    pub transaction: bool,
    /// The newest `status`.
    pub status: Option<Status>,
    /// The revision of the newest model published, if there's one.
    pub published: Option<Revision>,
    /// The newest `timeline`, and the revisions it was asked at.
    pub timeline: Option<TimelineAnswer>,
    timeline_read: Option<Running<TimelineAnswer>>,
    timeline_asked: Option<(Revision, Option<Revision>)>,
    /// A published timeline was refused, nothing being published: the next is read of the
    /// current model, which waits for its rebuild (off the UI thread).
    nothing_published: bool,
    /// What `check` says of each feature, and of the document (the gate's problems).
    pub said: BTreeMap<String, Said>,
    pub document_problems: Vec<Said>,
    /// A `check` read, and whether it's of a document the gate refused (whose problems are
    /// all the document's).
    check_read: Option<(bool, Running<CheckAnswer>)>,
    checked: Option<(Revision, Option<Revision>)>,
    pub edits: Edits,
    under: Vec<Under>,
    /// A `begin` sent and not yet answered: the next edit begins none.
    beginning: bool,
    pub disk_choice: DiskChoice,
    /// The feature selected in the timeline or the browser.
    pub selected: Option<FeatureId>,
    /// The browser's hidden rows, by their names: the tab's state, never saved (open item
    /// 29).
    pub hidden: std::collections::BTreeSet<String>,
    /// The last refusal a command of the tab's answered, as said to the person.
    pub message: Option<String>,
    /// The session has closed: the tab goes.
    pub closed: bool,
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
            published: None,
            timeline: None,
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
            beginning: false,
            disk_choice: DiskChoice::None,
            selected: None,
            hidden: std::collections::BTreeSet::new(),
            message: None,
            closed: false,
        }
    }

    /// The tab's name: its file's.
    pub fn name(&self) -> &str {
        std::path::Path::new(&self.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.path)
    }

    /// Whether it holds unsaved edits: a transaction is open (decision 1: begun at the first
    /// edit), or an edit is under way.
    pub fn unsaved(&self) -> bool {
        self.transaction || self.beginning || !self.edits.idle()
    }

    /// The session's state as it is now (`status` and the published model, each answered at
    /// once), every command under way polled, and the reads a change needs started.
    pub fn poll<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let status = sessions.status(&self.id);
        self.took_session(&status);
        match status.result {
            Ok(status) => self.status = Some(status),
            // Its session isn't open any more: the tab goes.
            Err(_) => {
                self.closed = true;
                return;
            }
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

    /// The session as `response` left it.
    fn took_session<T>(&mut self, response: &Response<T>) {
        if let Some(session) = &response.session {
            self.revision = session.revision;
            self.transaction = session.transaction;
        }
    }

    fn take_answers<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(read) = self.timeline_read.take() {
            match read.try_take() {
                Ok(response) => match response.result {
                    Ok(timeline) => {
                        // Marks read again only for a timeline with any; none, none said.
                        if !marked(&timeline) {
                            self.said.clear();
                        }
                        if timeline.problems == 0 {
                            self.document_problems.clear();
                        }
                        self.timeline = Some(timeline);
                    }
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
        self.take_edit(sessions);
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
                    match response.result {
                        Ok(_) => {
                            self.disk_choice = DiskChoice::None;
                            *close |= then_close;
                        }
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Commit {
                    running,
                    then_close,
                }),
            },
            Under::Rollback {
                running,
                then_close,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    match response.result {
                        Ok(_) => {
                            self.disk_choice = DiskChoice::None;
                            *close |= then_close;
                        }
                        Err(problems) => self.message = Some(said(&problems)),
                    }
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
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                    None
                }
                Err(running) => Some(Under::Step(running)),
            },
            Under::Close(running) => match running.try_take() {
                Ok(response) => {
                    match response.result {
                        Ok(Closed) => self.closed = true,
                        Err(problems) => self.message = Some(said(&problems)),
                    }
                    None
                }
                Err(running) => Some(Under::Close(running)),
            },
        }
    }

    /// The edit in flight polled, and, once it has answered, the next waiting sent against
    /// the revision its answer gave.
    fn take_edit<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(running) = self.edits.in_flight.take() {
            match running.try_take() {
                Ok(response) => {
                    self.took_session(&response);
                    if let Err(problems) = response.result {
                        self.message = Some(said(&problems));
                    }
                }
                Err(running) => {
                    self.edits.in_flight = Some(running);
                    return;
                }
            }
        }
        if self.edits.waiting.is_empty() {
            return;
        }
        let edit = self.edits.waiting.remove(0);
        self.edits.sent += 1;
        self.edits.in_flight = Some(sessions.set_field(
            &self.id,
            self.revision,
            edit.feature,
            edit.field,
            edit.value,
        ));
    }

    /// `edit` made by the person: inside the tab's transaction, begun first if none is open
    /// (decision 1), and sent now if no edit is in flight, or else waiting (section 3).
    pub fn edit<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, edit: Edit) {
        if !self.transaction && !self.beginning {
            self.beginning = true;
            self.under.push(Under::Begin(sessions.begin(&self.id)));
        }
        self.edits.push(edit);
        self.take_edit(sessions);
    }

    /// Save (`commit`), then, if `then_close`, close.
    pub fn save<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, then_close: bool) {
        self.under.push(Under::Commit {
            running: sessions.commit(&self.id),
            then_close,
        });
    }

    /// Revert (`rollback`): the file's text, byte for byte; then, if `then_close`, close.
    /// Also how a file changed on disk is reloaded with edits held: the session reloads it
    /// once the transaction has ended (section 4).
    pub fn revert<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, then_close: bool) {
        self.under.push(Under::Rollback {
            running: sessions.rollback(&self.id),
            then_close,
        });
    }

    /// Close, its edits saved or dropped first (`close` is refused inside a transaction,
    /// decision 22).
    pub fn close<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        self.under.push(Under::Close(sessions.close(&self.id)));
    }

    /// Undo or redo one edit of the transaction.
    pub fn step<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, undo: bool) {
        let running = if undo {
            sessions.undo(&self.id, self.revision, None)
        } else {
            sessions.redo(&self.id, self.revision, None)
        };
        self.under.push(Under::Step(running));
    }

    /// Stop what's running (`cancel`), if a command a client sent is: never the session's
    /// own reload, which nothing cancels.
    pub fn stop<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        if let Some(command) = self
            .status
            .as_ref()
            .and_then(|status| status.running)
            .and_then(RunningCommand::command)
        {
            let _ = sessions.cancel(&self.id, command);
        }
    }

    /// The choice section 4 offers once the file changed on disk while edits are held.
    fn offer_disk_choice(&mut self) {
        let disk = self.status.as_ref().and_then(|status| status.disk);
        let changed = matches!(disk, Some(Disk::Changed | Disk::Gone));
        self.disk_choice = match (self.disk_choice, changed, self.transaction, disk) {
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
        let model = if self.published.is_none() && self.nothing_published {
            ModelChoice::Current
        } else {
            ModelChoice::Published
        };
        self.timeline_asked = Some(asked);
        self.timeline_read = Some(sessions.timeline(&self.id, model));
    }

    /// The text of the marks read once a timeline says there are problems or warnings:
    /// from a `check` of the published model, or, for a document the gate refused, of the
    /// current one, which answers with the gate's problems.
    fn read_check<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        let Some(timeline) = &self.timeline else {
            return;
        };
        let asked = (timeline.revision, timeline.model);
        if self.check_read.is_some() || self.checked == Some(asked) || !marked(timeline) {
            return;
        }
        let gate = timeline.problems > 0;
        let model = if gate || timeline.model.is_none() {
            CheckModel::Current { fresh: false }
        } else {
            CheckModel::Published
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
