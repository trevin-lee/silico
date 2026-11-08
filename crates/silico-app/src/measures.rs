//! The selection view's measures (M5.7; the prototype's `selectionProps`,
//! docs/design/m5-ui/js/app.js): what the core's commands say of the elements selected,
//! never the app's own geometry. One element: its entry as `elements` lists it (its type,
//! area, length, ends, normal, axis, radius, position), found in its body's listing page by
//! page, by the name the selection holds it by (its own, or one of its aliases). Two:
//! `distance` (the least distance, and the normal, centre, and centre-to-element distances
//! where they apply) and `angle`, both asked at once; `angle`'s refusal of a pair it doesn't
//! measure is kept as its answer, for the view to leave its row out (SolidWorks' Measure
//! reports what applies to the entities selected: docs/ux/solidworks.md, §7). Anything else
//! (no element, more than two, or one the model doesn't find now) reads nothing.
//!
//! Each read is of the published model, on a thread of the core's, polled with `try_take`
//! and never waited on, so the UI thread never waits (invariant 4). The reads are keyed on
//! what they're of ([`Of`]): the selection's version, which moves with every change of its
//! items, and the published model its names were read against (the tab's count of the
//! models published, and its revision). So a pick or a rebuild reads again, and nothing else
//! does. A new key cancels the reads out and drops every answer held, so no answer of
//! another selection or model is given for this one; and an answer read from another
//! revision than its key's (a model published after it was asked) is dropped as it comes,
//! the rebuild that published that model changing the key. A Measure tool reads through the
//! same type.

use std::sync::Arc;

use silico_core::command::elements::{Cursor, ENTRIES_AT_MOST, Entry, Listed};
use silico_core::command::measure::{AngleAnswer, DistanceAnswer};
use silico_core::command::{
    ModelChoice, Problems, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::FeatureId;
use silico_core::kernel::Clients;
use silico_core::naming::Name;
use silico_core::{Error, ErrorCode};

use crate::selection::{Selection, State};

/// What a set of measures is of: the selection's version, and the published model its names
/// were read against (the tab's count of the models published, and its revision).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Of {
    pub version: u64,
    pub model: (u64, Revision),
}

impl Of {
    /// What `selection` is now, if it's read against a published model.
    pub fn now(selection: &Selection) -> Option<Of> {
        selection.model_of().map(|model| Of {
            version: selection.version(),
            model,
        })
    }
}

/// What's measured: one element, by the body it's in and the name the selection holds it
/// by; or two, by the names the selection holds them by, in the order picked.
#[derive(Debug, Clone, PartialEq)]
pub enum Measured {
    One { body: FeatureId, name: Arc<Name> },
    Two([Arc<Name>; 2]),
}

impl Measured {
    /// What `selection` asks to be measured: its one or two elements, each found in the model
    /// it was read against; nothing for none or more than two, or while one waits (its body
    /// not named yet, or its feature not built).
    pub fn of(selection: &Selection) -> Option<Measured> {
        // Three at most looked at: a third is enough to measure nothing.
        let found: Option<Vec<_>> = selection
            .elements()
            .take(3)
            .map(|picked| match &picked.state {
                State::Found(selected) => Some(selected),
                State::Waiting { .. } => None,
            })
            .collect();
        match found?.as_slice() {
            [one] => Some(Measured::One {
                body: one.feature.clone(),
                name: Arc::clone(&one.name),
            }),
            [a, b] => Some(Measured::Two([Arc::clone(&a.name), Arc::clone(&b.name)])),
            _ => None,
        }
    }
}

/// The core's answers for what was measured, each once it has come: the element's entry
/// (one element), or the distance and the angle (two).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answers {
    pub element: Option<Result<Entry, Problems>>,
    pub distance: Option<Result<DistanceAnswer, Problems>>,
    pub angle: Option<Result<AngleAnswer, Problems>>,
}

/// A page of the body's listing out, for the element named `name`.
struct Page {
    running: Running<Listed>,
    body: FeatureId,
    name: Arc<Name>,
}

/// A tab's measures of its selection: what they're of, the answers held, and the reads out.
pub struct Measures {
    of: Option<Of>,
    measured: Option<Measured>,
    answers: Answers,
    page: Option<Page>,
    distance: Option<Running<DistanceAnswer>>,
    angle: Option<Running<AngleAnswer>>,
    /// The most entries a page of the listing asks for: [`ENTRIES_AT_MOST`], or fewer in a
    /// test of the paging.
    limit: u32,
    /// How many reads it has started.
    reads: u64,
}

impl Default for Measures {
    fn default() -> Self {
        Measures {
            of: None,
            measured: None,
            answers: Answers::default(),
            page: None,
            distance: None,
            angle: None,
            limit: ENTRIES_AT_MOST,
            reads: 0,
        }
    }
}

impl Measures {
    /// Measures whose listing pages hold at most `limit` entries: a test's, to page a small
    /// body.
    #[cfg(test)]
    pub(crate) fn paged(limit: u32) -> Self {
        Measures {
            limit,
            ..Measures::default()
        }
    }

    /// As if `measured`, of `of`, had been read and answered `answers`, nothing out: a
    /// view's test's.
    #[cfg(test)]
    pub(crate) fn answered_as(&mut self, of: Of, measured: Option<Measured>, answers: Answers) {
        self.cancel();
        self.of = Some(of);
        self.measured = measured;
        self.answers = answers;
    }

    /// How many reads it has started, pages of a listing each one: what a test counts.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// What it's of, once asked.
    pub fn of(&self) -> Option<Of> {
        self.of
    }

    /// The answers held, if they're of `of`: none for another selection or model, so a
    /// view never shows another's.
    pub fn answers(&self, of: Of) -> Option<&Answers> {
        (self.of == Some(of)).then_some(&self.answers)
    }

    /// Whether a read is out.
    pub fn reading(&self) -> bool {
        self.page.is_some() || self.distance.is_some() || self.angle.is_some()
    }

    /// `measured`, of `of`, read through `sessions` of `session`'s published model, unless
    /// that's what was asked last: the reads out cancelled and every answer dropped, then
    /// what `measured` needs asked (nothing for none, or for no model).
    pub fn ask<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        of: Option<Of>,
        measured: Option<Measured>,
    ) {
        if self.of == of && self.measured == measured {
            return;
        }
        let _span = tracing::debug_span!("measures_ask", ?of).entered();
        self.cancel();
        self.of = of;
        self.measured = measured.clone();
        self.answers = Answers::default();
        if of.is_none() {
            return;
        }
        match measured {
            None => {}
            Some(Measured::One { body, name }) => self.list(sessions, session, body, name, None),
            Some(Measured::Two(names)) => {
                let names = names.map(|name| name.to_string());
                self.reads += 2;
                self.distance =
                    Some(sessions.distance(session, ModelChoice::Published, names.clone(), None));
                self.angle = Some(sessions.angle(session, ModelChoice::Published, names, None));
            }
        }
    }

    /// The reads out polled: each answer taken if it has come (one of another revision than
    /// asked dropped), and, for an element its page didn't hold, the next page asked.
    pub fn poll<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, session: &SessionId) {
        let Some(of) = self.of else {
            return;
        };
        if let Some(running) = self.distance.take() {
            match running.try_take() {
                Ok(response) => self.answers.distance = taken(of, response),
                Err(running) => self.distance = Some(running),
            }
        }
        if let Some(running) = self.angle.take() {
            match running.try_take() {
                Ok(response) => self.answers.angle = taken(of, response),
                Err(running) => self.angle = Some(running),
            }
        }
        let Some(page) = self.page.take() else {
            return;
        };
        let response = match page.running.try_take() {
            Ok(response) => response,
            Err(running) => {
                self.page = Some(Page { running, ..page });
                return;
            }
        };
        let listed = match taken(of, response) {
            None => return,
            Some(Err(problems)) => {
                self.answers.element = Some(Err(problems));
                return;
            }
            Some(Ok(listed)) => listed,
        };
        let named = |entry: &&Entry| {
            *entry.name == *page.name || entry.aliases.iter().any(|alias| *alias.name == *page.name)
        };
        if let Some(entry) = listed.entries.iter().find(named) {
            self.answers.element = Some(Ok(entry.clone()));
            return;
        }
        match listed.next {
            Some(next) => self.list(sessions, session, page.body, page.name, Some(next)),
            None => self.answers.element = Some(Err(unlisted(&page.name, &page.body))),
        }
    }

    /// A page of `body`'s elements asked, after `after`, to find the one named `name` in.
    fn list<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        body: FeatureId,
        name: Arc<Name>,
        after: Option<Cursor>,
    ) {
        self.reads += 1;
        let running = sessions.elements(
            session,
            ModelChoice::Published,
            Some(body.clone()),
            after,
            self.limit,
        );
        self.page = Some(Page {
            running,
            body,
            name,
        });
    }

    /// Every read out cancelled and let go: its answer, if it comes, goes nowhere.
    fn cancel(&mut self) {
        if let Some(page) = self.page.take() {
            page.running.cancel();
        }
        if let Some(running) = self.distance.take() {
            running.cancel();
        }
        if let Some(running) = self.angle.take() {
            running.cancel();
        }
    }
}

/// `response`'s result, if it's of `of`'s model: none for one read from another revision (a
/// model published after it was asked), which the rebuild that published it asks again.
fn taken<T>(of: Of, response: Response<T>) -> Option<Result<T, Problems>> {
    match &response.model {
        Some(model) if model.revision != of.model.1 => {
            tracing::debug!(
                asked = of.model.1.number(),
                read = model.revision.number(),
                "a measure read of another model: dropped"
            );
            None
        }
        _ => Some(response.result),
    }
}

/// Why an element the selection found in `body` isn't in its listing: the selection and the
/// listing read one model, so it's a bug, said as one.
fn unlisted(name: &Name, body: &FeatureId) -> Problems {
    Error::new(
        ErrorCode::Internal,
        format!("{name} was found in {body}'s solid, but its listing doesn't hold it"),
    )
    .with_element(name.to_string())
    .with_suggestion("select it again; if it's still not measured, report it")
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::How;
    use silico_core::document::Length;
    use silico_core::fake::FakeKernel;
    use silico_core::kernel::Op;

    type Fake = Arc<FakeKernel>;

    /// A 4 by 2 by 1 block, opened on the fake kernel and rebuilt, and a selection read
    /// against its published model: its 6 faces, 12 edges, and 8 vertices, each described
    /// by the fake as standing for its number `n` (a face of area 1 centred at (n, 0, 0), an
    /// edge 1 long from (n, 0, 0), a vertex at (n, 0, 0)), each face of type other.
    struct Opened {
        _dir: tempfile::TempDir,
        kernel: Fake,
        sessions: Arc<Sessions<Fake>>,
        id: SessionId,
        selection: Selection,
    }

    fn opened() -> Opened {
        opened_with(&[block("k7f2qx3m", 0, 4)], |_| {})
    }

    /// A box `id`, `length` by 2 by 1, from (`x`, 0, 0), as a document writes it.
    fn block(id: &str, x: u32, length: u32) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": 2, "height": 1}}"#
        )
    }

    /// [`opened`]'s, of a document of `features`, on a fake kernel `setup` prepares.
    fn opened_with(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Opened {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("part.silico");
        std::fs::write(
            &document,
            format!(
                r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
                features.join(", ")
            ),
        )
        .unwrap();
        let fake = FakeKernel::new(1).unwrap();
        setup(&fake);
        let kernel = Arc::new(fake);
        let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let id = sessions.open(&document, false).wait().session.unwrap().id;
        sessions
            .timeline(&id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        let published = sessions.published(&id).result.unwrap();
        // Every feature's elements named, as the background names them.
        for (feature, _) in published.model.features() {
            published.model.element_map(feature);
        }
        let mut selection = Selection::default();
        selection.published(Some((published.model, published.revision)), 1);
        Opened {
            _dir: dir,
            kernel,
            sessions,
            id,
            selection,
        }
    }

    impl Opened {
        /// `names` selected, in order.
        fn select(&mut self, names: &[&str]) {
            self.selection.clear();
            for name in names {
                self.selection
                    .pick(&format!("k7f2qx3m.box.{name}"), How::Toggle);
            }
            assert_eq!(self.selection.elements().count(), names.len());
        }

        /// The selection as it is now asked of `measures`.
        fn ask(&self, measures: &mut Measures) -> Of {
            let of = Of::now(&self.selection).unwrap();
            measures.ask(
                &self.sessions,
                &self.id,
                Some(of),
                Measured::of(&self.selection),
            );
            of
        }

        /// `measures` polled until `done` holds, within a minute.
        fn polled_until(&self, measures: &mut Measures, done: impl Fn(&Measures) -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !done(measures) {
                measures.poll(&self.sessions, &self.id);
                assert!(std::time::Instant::now() < deadline, "never answered");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// Whether the fake's request `op` is the distance between faces `a` and `b`.
    fn between(op: &Op, a: u32, b: u32) -> bool {
        matches!(op, Op::Distance { first, second, .. } if (first.index, second.index) == (a, b))
    }

    #[test]
    fn one_element_is_found_on_the_page_of_its_bodys_listing_that_holds_it() {
        let mut opened = opened();
        // Pages of 5: the block's 26 entries are faces, edges, then vertices, each by name,
        // so Vertex8 is the 26th, on the 6th page; Face1 the 1st, on the first.
        let mut measures = Measures::paged(5);
        for (name, pages, at) in [
            ("Vertex8", 6, [8.0, 0.0, 0.0]),
            ("Face1", 1, [1.0, 0.0, 0.0]),
        ] {
            opened.select(&[name]);
            let before = measures.reads();
            let of = opened.ask(&mut measures);
            opened.polled_until(&mut measures, |m| {
                m.answers(of)
                    .is_some_and(|answers| answers.element.is_some())
            });
            let answers = measures.answers(of).unwrap();
            let entry = answers.element.clone().unwrap().unwrap();
            assert_eq!(entry.name.to_string(), format!("k7f2qx3m.box.{name}"));
            assert_eq!(measures.reads() - before, pages, "{name}");
            // The fake's vertex n is at (n, 0, 0); its face n is centred there.
            let point = entry.position.or(entry.centroid).unwrap().map(Length::mm);
            assert_eq!(point, at, "{name}");
            assert_eq!(answers.distance, None);
            assert_eq!(answers.angle, None);
            assert!(!measures.reading());
        }
    }

    #[test]
    fn two_elements_are_measured_by_distance_and_angle_and_an_angle_refused_is_its_answer() {
        let mut opened = opened();
        opened.select(&["Face1", "Face3"]);
        let mut measures = Measures::default();
        let of = opened.ask(&mut measures);
        assert_eq!(measures.reads(), 2);
        opened.polled_until(&mut measures, |m| !m.reading());
        let answers = measures.answers(of).unwrap();
        // The fake's faces 1 and 3 stand at (1, 0, 0) and (3, 0, 0): 2 apart.
        let distance = answers.distance.clone().unwrap().unwrap();
        assert_eq!(distance.distance, Length::from_nm(2_000_000));
        assert_eq!(distance.normal_distance, None);
        // Its faces are of type other: `angle` refuses them, as not planar.
        let refused = answers.angle.clone().unwrap().unwrap_err();
        assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
        assert_eq!(answers.element, None);
        // Asked again as it is: nothing is read again.
        opened.ask(&mut measures);
        assert_eq!(measures.reads(), 2);
    }

    #[test]
    fn three_elements_read_nothing_and_none_reads_nothing() {
        let mut opened = opened();
        let mut measures = Measures::default();
        opened.select(&["Face1", "Face2", "Edge1"]);
        let of = opened.ask(&mut measures);
        assert_eq!(measures.reads(), 0);
        assert_eq!(measures.answers(of), Some(&Answers::default()));
        opened.select(&[]);
        opened.ask(&mut measures);
        assert_eq!(measures.reads(), 0);
        assert_eq!(Measured::of(&opened.selection), None);
    }

    #[test]
    fn a_new_selection_drops_every_answer_and_an_old_reads_answer_is_never_given_for_it() {
        let mut opened = opened();
        let mut measures = Measures::default();
        // Faces 1 and 2, 1 apart, answered.
        opened.select(&["Face1", "Face2"]);
        let first = opened.ask(&mut measures);
        opened.polled_until(&mut measures, |m| !m.reading());
        let one = Length::from_nm(1_000_000);
        let distance_of = |m: &Measures, of: Of| {
            m.answers(of)
                .and_then(|answers| answers.distance.clone())
                .map(|distance| distance.unwrap().distance)
        };
        assert_eq!(distance_of(&measures, first), Some(one));
        // Faces 3 and 5, their distance held: none is given, never the last selection's.
        let hold = opened.kernel.hold(|op| between(op, 3, 5));
        opened.select(&["Face3", "Face5"]);
        let second = opened.ask(&mut measures);
        assert_ne!(first, second);
        assert_eq!(measures.answers(first), None);
        for _ in 0..20 {
            measures.poll(&opened.sessions, &opened.id);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(distance_of(&measures, second), None);
        hold.release();
        opened.polled_until(&mut measures, |m| distance_of(m, second).is_some());
        assert_eq!(
            distance_of(&measures, second),
            Some(Length::from_nm(2_000_000))
        );
        // Faces 1 and 2 again, held; then faces 4 and 6, 2 apart, answered: the held read,
        // let go, never answers for them.
        let hold = opened.kernel.hold(|op| between(op, 1, 2));
        opened.select(&["Face1", "Face2"]);
        opened.ask(&mut measures);
        opened.select(&["Face4", "Face6"]);
        let last = opened.ask(&mut measures);
        opened.polled_until(&mut measures, |m| !m.reading());
        hold.release();
        for _ in 0..20 {
            measures.poll(&opened.sessions, &opened.id);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            distance_of(&measures, last),
            Some(Length::from_nm(2_000_000))
        );
    }

    #[test]
    fn an_answer_read_from_another_revision_than_asked_is_dropped_and_not_asked_again() {
        let mut opened = opened();
        opened.select(&["Face1", "Face2"]);
        let mut measures = Measures::default();
        // As if the selection were read against revision 9, which the session hasn't: the
        // published model the reads find is of another.
        let real = Of::now(&opened.selection).unwrap();
        let of = Of {
            model: (real.model.0, Revision::from_number(9).unwrap()),
            ..real
        };
        measures.ask(
            &opened.sessions,
            &opened.id,
            Some(of),
            Measured::of(&opened.selection),
        );
        opened.polled_until(&mut measures, |m| !m.reading());
        assert_eq!(measures.answers(of), Some(&Answers::default()));
        for _ in 0..5 {
            measures.poll(&opened.sessions, &opened.id);
        }
        assert_eq!(measures.reads(), 2);
        assert!(!measures.reading());
    }
}
