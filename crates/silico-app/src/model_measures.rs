//! The document's measures (the prototype's `docProps`, docs/design/m5-ui/js/app.js: its
//! MODEL section's Volume, Area, Size, and Faces): what the core's commands say of the
//! published model's bodies, never the app's own geometry. Volume and Area are
//! `mass_properties`' of every body, added (each as the body's fingerprint holds it, to a
//! millionth); Faces is how many faces `query` matches across every body; Size is the box
//! around every body's tight box, as the `meshes` read the viewport holds gives each
//! (`crate::viewport::ViewBody::bounds`, the kernel's box of the solid, not of its mesh).
//!
//! SEAM: Size waits on the viewport's read of the same model, so a view the core won't mesh
//! (an eye inside a solid) leaves it not measured. The live model's boxes come with no other
//! read: `mass_properties` drops the box its summary holds, and `outline`'s and `text`'s are
//! the stored fingerprints', a transaction's begin's until its commit. A box in
//! `mass_properties`' answer would free it, a change of its contract (the maintainer's).
//!
//! Each read is of the published model, on a thread of the core's, polled with `try_take`
//! and never waited on, so the UI thread never waits (invariant 4). The reads are keyed on
//! the published model (the tab's count of the models published, and its revision), as the
//! selection's measures are (`crate::measures`): a new model cancels the reads out and drops
//! the answers held, and an answer read from another revision than its key's is dropped as
//! it comes. A read refused is asked again after a delay that grows with each refusal in a
//! row, as the viewport asks its meshes again (`crate::viewport::retry_after`): a refusal
//! for the moment (the session's published reads all running) passes, and one of the model
//! (a solid that couldn't be described) is asked again at most
//! [`RETRY_MOST`](crate::viewport::RETRY_MOST) apart, its refusal shown meanwhile. They're asked
//! only while the property panel shows the document's properties.

use std::sync::Arc;
use std::time::Instant;

use silico_core::command::elements::{Queried, Query};
use silico_core::command::measure::{Bodies, MassProperties};
use silico_core::command::{ModelChoice, Problems, Revision, Running, SessionId, Sessions};
use silico_core::document::{FeatureId, Length, Measure};
use silico_core::kernel::{Clients, ElementKind};

/// The published model a set of measures is of: the tab's count of the models published,
/// and its revision.
pub type ModelKey = (u64, Revision);

/// The core's answers for the model, each once it has come: its bodies' mass properties, and
/// how many faces its bodies have.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    pub bodies: Option<Result<Bodies, Problems>>,
    pub faces: Option<Result<Queried, Problems>>,
}

impl Answers {
    /// The bodies' volumes added, once answered.
    pub fn volume(&self) -> Option<Measure> {
        self.added(|body| body.volume)
    }

    /// The bodies' areas added, once answered.
    pub fn area(&self) -> Option<Measure> {
        self.added(|body| body.area)
    }

    /// `measure` of every body, added; none until answered, or for a sum past what a measure
    /// holds.
    fn added(&self, measure: impl Fn(&MassProperties) -> Measure) -> Option<Measure> {
        let Some(Ok(bodies)) = &self.bodies else {
            return None;
        };
        bodies
            .bodies
            .iter()
            .try_fold(0_i128, |sum, body| {
                sum.checked_add(measure(body).millionths())
            })
            .map(Measure::from_millionths)
    }

    /// How many faces every body has, once answered.
    pub fn face_count(&self) -> Option<u64> {
        match &self.faces {
            Some(Ok(queried)) => Some(queried.matched),
            _ => None,
        }
    }

    /// The bodies measured, by feature, in timeline order, once answered.
    pub fn measured_features(&self) -> Option<Vec<&FeatureId>> {
        match &self.bodies {
            Some(Ok(bodies)) => Some(bodies.bodies.iter().map(|body| &body.feature).collect()),
            _ => None,
        }
    }

    /// Whether the mass properties' read was refused.
    pub fn bodies_refused(&self) -> bool {
        matches!(self.bodies, Some(Err(_)))
    }

    /// Whether the faces' read was refused.
    pub fn faces_refused(&self) -> bool {
        matches!(self.faces, Some(Err(_)))
    }

    /// The first refusal of either read, if one was refused: the mass properties', then the
    /// faces'.
    pub fn refused(&self) -> Option<&Problems> {
        [
            self.bodies
                .as_ref()
                .and_then(|bodies| bodies.as_ref().err()),
            self.faces.as_ref().and_then(|faces| faces.as_ref().err()),
        ]
        .into_iter()
        .flatten()
        .next()
    }
}

/// A tab's measures of its published model: what they're of, the answers held, the reads
/// out, and when a read refused is asked again.
#[derive(Default)]
pub struct ModelMeasures {
    of: Option<ModelKey>,
    answers: Answers,
    bodies: Option<Running<Bodies>>,
    faces: Option<Running<Queried>>,
    /// When the reads refused are asked again, once one is.
    retry_at: Option<Instant>,
    /// How many refusals came in a row for this model: what the next delay grows by.
    refusals: u32,
    /// How many reads it has started.
    reads: u64,
}

impl ModelMeasures {
    /// As if the model `of` had been read and answered `answers`, nothing out: a view's
    /// test's.
    #[cfg(test)]
    pub(crate) fn answered_as(&mut self, of: ModelKey, answers: Answers) {
        self.cancel();
        self.of = Some(of);
        self.answers = answers;
    }

    /// How many reads it has started: what a test counts.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// The answers held, if they're of `of`: none for another model, so the panel never
    /// shows another's.
    pub fn answers(&self, of: ModelKey) -> Option<&Answers> {
        (self.of == Some(of)).then_some(&self.answers)
    }

    /// Whether a read is out.
    pub fn reading(&self) -> bool {
        self.bodies.is_some() || self.faces.is_some()
    }

    /// When the reads refused are asked again, if one was refused.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }

    /// The model `of` measured through `sessions`, of `session`'s published model, at
    /// `now`: if it's another than was asked last, the reads out cancelled and every answer
    /// dropped, then both reads asked (nothing for no model); if it's the same, each read
    /// refused asked again once its delay is up, its refusal held till its answer comes.
    pub fn ask<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        of: Option<ModelKey>,
        now: Instant,
    ) {
        if self.of == of {
            if self.retry_at.is_some_and(|at| at <= now) && !self.reading() {
                let _span = tracing::debug_span!("model_measures_retry", ?of).entered();
                self.retry_at = None;
                if self.answers.bodies_refused() {
                    self.ask_bodies(sessions, session);
                }
                if self.answers.faces_refused() {
                    self.ask_faces(sessions, session);
                }
            }
            return;
        }
        let _span = tracing::debug_span!("model_measures_ask", ?of).entered();
        self.cancel();
        self.of = of;
        self.answers = Answers::default();
        self.retry_at = None;
        self.refusals = 0;
        if of.is_none() {
            return;
        }
        self.ask_bodies(sessions, session);
        self.ask_faces(sessions, session);
    }

    fn ask_bodies<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, id: &SessionId) {
        self.reads += 1;
        self.bodies = Some(sessions.mass_properties(id, ModelChoice::Published, None));
    }

    fn ask_faces<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, id: &SessionId) {
        self.reads += 1;
        // One entry is enough: `matched` counts every face before the limit.
        self.faces =
            Some(sessions.query(id, ModelChoice::Published, Query::of(ElementKind::Face, 1)));
    }

    /// The reads out polled at `now`: each answer taken if it has come (one of another
    /// revision than asked dropped). A refusal taken sets when the reads refused are asked
    /// again.
    pub fn poll(&mut self, now: Instant) {
        let Some(of) = self.of else {
            return;
        };
        let mut refused = false;
        if let Some(running) = self.bodies.take() {
            match running.try_take() {
                Ok(response) => {
                    self.answers.bodies = crate::measures::taken(of.1, response);
                    refused |= self.answers.bodies_refused();
                }
                Err(running) => self.bodies = Some(running),
            }
        }
        if let Some(running) = self.faces.take() {
            match running.try_take() {
                Ok(response) => {
                    self.answers.faces = crate::measures::taken(of.1, response);
                    refused |= self.answers.faces_refused();
                }
                Err(running) => self.faces = Some(running),
            }
        }
        // Once for the reads refused together: the delay grows by refusals in a row, which
        // only a new model starts again (both answered, nothing more is asked of this one).
        if refused && self.retry_at.is_none() {
            self.retry_at = Some(now + crate::viewport::retry_after(self.refusals));
            self.refusals = self.refusals.saturating_add(1);
        }
    }

    /// Every read out cancelled and let go: its answer, if it comes, goes nowhere.
    fn cancel(&mut self) {
        if let Some(running) = self.bodies.take() {
            running.cancel();
        }
        if let Some(running) = self.faces.take() {
            running.cancel();
        }
    }
}

/// The box around every one of `boxes`, each `[low, high]`, as its sides' lengths, rounded
/// to the nanometre: none for no box, or for a side a length can't hold.
pub fn size(boxes: impl IntoIterator<Item = [[f64; 3]; 2]>) -> Option<[Length; 3]> {
    let mut boxes = boxes.into_iter();
    let [mut low, mut high] = boxes.next()?;
    for [l, h] in boxes {
        for axis in 0..3 {
            low[axis] = low[axis].min(l[axis]);
            high[axis] = high[axis].max(h[axis]);
        }
    }
    let side = |axis: usize| Length::rounded(high[axis] - low[axis]);
    Some([side(0)?, side(1)?, side(2)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::fake::FakeKernel;

    type Fake = Arc<FakeKernel>;

    /// A document of `features` opened on the fake kernel and rebuilt: the sessions, the
    /// session, and its published model's key (one model seen published).
    struct Opened {
        _dir: tempfile::TempDir,
        sessions: Arc<Sessions<Fake>>,
        id: SessionId,
        of: ModelKey,
    }

    /// A box `id`, `length` by `width` by `height`, from (`x`, 0, 0), as a document writes it.
    fn block(id: &str, x: u32, [length, width, height]: [u32; 3]) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": {width}, "height": {height}}}"#
        )
    }

    /// `measures` polled at `now` until neither read is out, within a minute.
    fn polled_at(measures: &mut ModelMeasures, now: Instant) {
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        while measures.reading() {
            measures.poll(now);
            assert!(Instant::now() < deadline, "never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn opened(features: &[String]) -> Opened {
        opened_on(features, |_| {})
    }

    /// [`opened`], on a fake kernel `setup` prepares.
    fn opened_on(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Opened {
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
        let sessions = Arc::new(Sessions::with_cache_opener(kernel, |dir| {
            silico_core::cache::Cache::unsealed(dir)
        }));
        let id = sessions.open(&document, false).wait().session.unwrap().id;
        sessions
            .timeline(&id, ModelChoice::Current)
            .wait()
            .result
            .unwrap();
        let revision = sessions.published(&id).result.unwrap().revision;
        Opened {
            _dir: dir,
            sessions,
            id,
            of: (1, revision),
        }
    }

    impl Opened {
        /// `measures` asked of the model and polled until neither read is out, within a
        /// minute.
        fn measured(&self, measures: &mut ModelMeasures) {
            let now = Instant::now();
            measures.ask(&self.sessions, &self.id, Some(self.of), now);
            polled_at(measures, now);
        }
    }

    #[test]
    fn two_bodies_volumes_areas_and_faces_are_added() {
        // By hand: a 4 × 2 × 1 block, 8 mm³ and 2·(8 + 4 + 2) = 28 mm²; a 3 × 5 × 7 block,
        // 105 mm³ and 2·(15 + 21 + 35) = 142 mm²; 6 faces each.
        let opened = opened(&[
            block("k7f2qx3m", 0, [4, 2, 1]),
            block("b0000001", 10, [3, 5, 7]),
        ]);
        let mut measures = ModelMeasures::default();
        opened.measured(&mut measures);
        let answers = measures.answers(opened.of).unwrap();
        assert_eq!(
            answers.volume(),
            Some(Measure::from_millionths(113_000_000))
        );
        assert_eq!(answers.area(), Some(Measure::from_millionths(170_000_000)));
        assert_eq!(answers.face_count(), Some(12));
        assert_eq!(
            answers
                .measured_features()
                .unwrap()
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            ["k7f2qx3m", "b0000001"]
        );
        assert!(answers.refused().is_none());
        assert_eq!(measures.reads(), 2);
        // Asked again of the same model: nothing more read, the answers kept.
        measures.ask(
            &opened.sessions,
            &opened.id,
            Some(opened.of),
            Instant::now(),
        );
        assert_eq!(measures.reads(), 2);
        assert!(measures.answers(opened.of).unwrap().volume().is_some());
    }

    #[test]
    fn another_models_answers_are_never_given_and_a_new_model_reads_again() {
        let opened = opened(&[block("k7f2qx3m", 0, [4, 2, 1])]);
        let mut measures = ModelMeasures::default();
        opened.measured(&mut measures);
        // The same revision published again (a fresh check's): another model.
        let again = (2, opened.of.1);
        assert!(measures.answers(again).is_none());
        measures.ask(&opened.sessions, &opened.id, Some(again), Instant::now());
        assert_eq!(measures.reads(), 4);
        assert!(
            measures.answers(opened.of).is_none(),
            "the old answers dropped"
        );
        // No model: nothing asked, nothing held.
        measures.ask(&opened.sessions, &opened.id, None, Instant::now());
        assert!(!measures.reading());
        assert_eq!(measures.reads(), 4);
        assert!(measures.answers(again).is_none());
    }

    #[test]
    fn a_new_model_holds_none_of_the_old_ones_answers_before_its_own_come() {
        let opened = opened(&[block("k7f2qx3m", 0, [4, 2, 1])]);
        let mut measures = ModelMeasures::default();
        opened.measured(&mut measures);
        let again = (2, opened.of.1);
        measures.ask(&opened.sessions, &opened.id, Some(again), Instant::now());
        // Not polled: nothing of the new model has come, and nothing of the old is given.
        let answers = measures.answers(again).unwrap();
        assert!(answers.bodies.is_none() && answers.faces.is_none());
    }

    #[test]
    fn a_read_refused_is_asked_again_alone_after_a_delay_that_doubles() {
        use crate::viewport::RETRY_FIRST;
        // The kernel refuses to describe the block: `query` is refused every time, and
        // `mass_properties`, which asks no description, answers.
        let opened = opened_on(&[block("k7f2qx3m", 0, [4, 2, 1])], |kernel| {
            kernel.refuse(|op| {
                matches!(op, silico_core::kernel::Op::DescribeAll { .. }).then(|| {
                    silico_core::Error::new(
                        silico_core::ErrorCode::KernelFailed,
                        "the worker couldn't describe it",
                    )
                })
            });
        });
        let start = Instant::now();
        let mut measures = ModelMeasures::default();
        measures.ask(&opened.sessions, &opened.id, Some(opened.of), start);
        polled_at(&mut measures, start);
        assert_eq!(measures.reads(), 2);
        assert_eq!(measures.retry_at(), Some(start + RETRY_FIRST));
        // Before its delay: nothing asked.
        let early = start + RETRY_FIRST - std::time::Duration::from_millis(1);
        measures.ask(&opened.sessions, &opened.id, Some(opened.of), early);
        assert!(!measures.reading());
        assert_eq!(measures.reads(), 2);
        // At it: the faces' read alone asked again, its refusal shown till it answers; the
        // volume kept.
        let due = start + RETRY_FIRST;
        measures.ask(&opened.sessions, &opened.id, Some(opened.of), due);
        assert_eq!(measures.reads(), 3);
        assert!(measures.answers(opened.of).unwrap().faces_refused());
        assert!(measures.answers(opened.of).unwrap().volume().is_some());
        polled_at(&mut measures, due);
        // Refused again: twice the delay.
        assert_eq!(measures.retry_at(), Some(due + 2 * RETRY_FIRST));
        let answers = measures.answers(opened.of).unwrap();
        assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
        assert!(answers.faces_refused() && !answers.bodies_refused());
        // Another model: the refusals in a row start again.
        let again = (2, opened.of.1);
        let later = due + RETRY_FIRST;
        measures.ask(&opened.sessions, &opened.id, Some(again), later);
        polled_at(&mut measures, later);
        assert_eq!(measures.retry_at(), Some(later + RETRY_FIRST));
    }

    #[test]
    fn an_answer_of_another_revision_is_dropped() {
        let opened = opened(&[block("k7f2qx3m", 0, [4, 2, 1])]);
        let mut measures = ModelMeasures::default();
        // Asked as if of a revision the session never published: each answer, of the
        // published one, is dropped.
        let other = (1, Revision::from_number(99).unwrap());
        assert_ne!(other.1, opened.of.1);
        let now = Instant::now();
        measures.ask(&opened.sessions, &opened.id, Some(other), now);
        polled_at(&mut measures, now);
        let answers = measures.answers(other).unwrap();
        assert!(answers.bodies.is_none() && answers.faces.is_none());
    }

    #[test]
    fn a_size_is_the_box_around_every_box() {
        let mm = |v: f64| Length::rounded(v).unwrap();
        assert_eq!(size([]), None);
        // One box, from (1, 2, 3) to (5, 4, 3.5): 4 × 2 × 0.5.
        assert_eq!(
            size([[[1.0, 2.0, 3.0], [5.0, 4.0, 3.5]]]),
            Some([mm(4.0), mm(2.0), mm(0.5)])
        );
        // Two apart: the box from (-2, 0, 0) to (10, 7, 3), each side the farthest apart
        // ends, whichever box holds them.
        assert_eq!(
            size([
                [[0.0, 0.0, 0.0], [4.0, 7.0, 1.0]],
                [[-2.0, 1.0, 2.0], [10.0, 2.0, 3.0]],
            ]),
            Some([mm(12.0), mm(7.0), mm(3.0)])
        );
        // A side a length can't hold: none.
        assert_eq!(size([[[0.0; 3], [f64::INFINITY, 1.0, 1.0]]]), None);
        assert_eq!(size([[[0.0; 3], [f64::NAN, 1.0, 1.0]]]), None);
    }

    #[test]
    fn a_sum_past_what_a_measure_holds_is_none() {
        use silico_core::command::measure::MassProperties;
        let body = |volume: i128| MassProperties {
            feature: FeatureId::new("k7f2qx3m").unwrap(),
            volume: Measure::from_millionths(volume),
            area: Measure::from_millionths(1),
            center_of_mass: [Length::from_nm(0); 3],
        };
        let answers = Answers {
            bodies: Some(Ok(Bodies {
                bodies: vec![body(i128::MAX), body(1)],
            })),
            faces: None,
        };
        assert_eq!(answers.volume(), None);
        assert_eq!(answers.area(), Some(Measure::from_millionths(2)));
        assert_eq!(answers.face_count(), None);
    }
}
