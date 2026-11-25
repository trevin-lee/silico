//! The document's measures (the prototype's `docProps`, docs/design/m5-ui/js/app.js: its
//! MODEL section's Volume, Area, Size, and Faces): what the core's commands say of the
//! published model's bodies, never the app's own geometry. Volume and Area are
//! `mass_properties`' of every body, added (each as the body's fingerprint holds it, to a
//! millionth); Faces is how many faces `query` matches across every body; Size is the box
//! around every body's tight box, as the `meshes` read the viewport holds gives each
//! (`crate::viewport::ViewBody::bounds`, the kernel's box of the solid, not of its mesh), since
//! no element read gives a solid's box.
//!
//! Each read is of the published model, on a thread of the core's, polled with `try_take`
//! and never waited on, so the UI thread never waits (invariant 4). The reads are keyed on
//! the published model (the tab's count of the models published, and its revision), as the
//! selection's measures are (`crate::measures`): a new model cancels the reads out and drops
//! the answers held, and an answer read from another revision than its key's is dropped as
//! it comes. They're asked only while the property panel shows the document's properties.

use std::sync::Arc;

use silico_core::command::elements::{Queried, Query};
use silico_core::command::measure::Bodies;
use silico_core::command::{
    ModelChoice, Problems, Response, Revision, Running, SessionId, Sessions,
};
use silico_core::document::{FeatureId, Length, Measure};
use silico_core::kernel::{Clients, ElementKind};

/// The published model a set of measures is of: the tab's count of the models published,
/// and its revision.
pub type Of = (u64, Revision);

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
    fn added(
        &self,
        measure: impl Fn(&silico_core::command::measure::MassProperties) -> Measure,
    ) -> Option<Measure> {
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
    pub fn faces(&self) -> Option<u64> {
        match &self.faces {
            Some(Ok(queried)) => Some(queried.matched),
            _ => None,
        }
    }

    /// The bodies measured, by feature, in timeline order, once answered.
    pub fn bodies(&self) -> Option<Vec<&FeatureId>> {
        match &self.bodies {
            Some(Ok(bodies)) => Some(bodies.bodies.iter().map(|body| &body.feature).collect()),
            _ => None,
        }
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

/// A tab's measures of its published model: what they're of, the answers held, and the
/// reads out.
#[derive(Default)]
pub struct ModelMeasures {
    of: Option<Of>,
    answers: Answers,
    bodies: Option<Running<Bodies>>,
    faces: Option<Running<Queried>>,
    /// How many reads it has started.
    reads: u64,
}

impl ModelMeasures {
    /// As if the model `of` had been read and answered `answers`, nothing out: a view's
    /// test's.
    #[cfg(test)]
    pub(crate) fn answered_as(&mut self, of: Of, answers: Answers) {
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
    pub fn answers(&self, of: Of) -> Option<&Answers> {
        (self.of == Some(of)).then_some(&self.answers)
    }

    /// Whether a read is out.
    pub fn reading(&self) -> bool {
        self.bodies.is_some() || self.faces.is_some()
    }

    /// The model `of` measured through `sessions`, of `session`'s published model, unless
    /// that's what was asked last: the reads out cancelled and every answer dropped, then
    /// both reads asked (nothing for no model).
    pub fn ask<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        of: Option<Of>,
    ) {
        if self.of == of {
            return;
        }
        let _span = tracing::debug_span!("model_measures_ask", ?of).entered();
        self.cancel();
        self.of = of;
        self.answers = Answers::default();
        if of.is_none() {
            return;
        }
        self.reads += 2;
        self.bodies = Some(sessions.mass_properties(session, ModelChoice::Published, None));
        // One entry is enough: `matched` counts every face before the limit.
        self.faces = Some(sessions.query(
            session,
            ModelChoice::Published,
            Query::of(ElementKind::Face, 1),
        ));
    }

    /// The reads out polled: each answer taken if it has come (one of another revision than
    /// asked dropped).
    pub fn poll(&mut self) {
        let Some(of) = self.of else {
            return;
        };
        if let Some(running) = self.bodies.take() {
            match running.try_take() {
                Ok(response) => self.answers.bodies = taken(of, response),
                Err(running) => self.bodies = Some(running),
            }
        }
        if let Some(running) = self.faces.take() {
            match running.try_take() {
                Ok(response) => self.answers.faces = taken(of, response),
                Err(running) => self.faces = Some(running),
            }
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

/// `response`'s result, if it's of `of`'s model: none for one read from another revision (a
/// model published after it was asked), which the rebuild that published it asks again.
fn taken<T>(of: Of, response: Response<T>) -> Option<Result<T, Problems>> {
    match &response.model {
        Some(model) if model.revision != of.1 => {
            tracing::debug!(
                asked = of.1.number(),
                read = model.revision.number(),
                "a model measure read of another model: dropped"
            );
            None
        }
        _ => Some(response.result),
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
        of: Of,
    }

    /// A box `id`, `length` by `width` by `height`, from (`x`, 0, 0), as a document writes it.
    fn block(id: &str, x: u32, [length, width, height]: [u32; 3]) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": {width}, "height": {height}}}"#
        )
    }

    fn opened(features: &[String]) -> Opened {
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
        let kernel = Arc::new(FakeKernel::new(1).unwrap());
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
            measures.ask(&self.sessions, &self.id, Some(self.of));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while measures.reading() {
                measures.poll();
                assert!(std::time::Instant::now() < deadline, "never answered");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
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
        assert_eq!(answers.faces(), Some(12));
        assert_eq!(
            answers
                .bodies()
                .unwrap()
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            ["k7f2qx3m", "b0000001"]
        );
        assert!(answers.refused().is_none());
        assert_eq!(measures.reads(), 2);
        // Asked again of the same model: nothing more read, the answers kept.
        measures.ask(&opened.sessions, &opened.id, Some(opened.of));
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
        measures.ask(&opened.sessions, &opened.id, Some(again));
        assert_eq!(measures.reads(), 4);
        assert!(
            measures.answers(opened.of).is_none(),
            "the old answers dropped"
        );
        // No model: nothing asked, nothing held.
        measures.ask(&opened.sessions, &opened.id, None);
        assert!(!measures.reading());
        assert_eq!(measures.reads(), 4);
        assert!(measures.answers(again).is_none());
    }

    #[test]
    fn an_answer_of_another_revision_is_dropped() {
        let opened = opened(&[block("k7f2qx3m", 0, [4, 2, 1])]);
        let mut measures = ModelMeasures::default();
        // Asked as if of a revision the session never published: each answer, of the
        // published one, is dropped.
        let other = (1, Revision::from_number(99).unwrap());
        assert_ne!(other.1, opened.of.1);
        measures.ask(&opened.sessions, &opened.id, Some(other));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while measures.reading() {
            measures.poll();
            assert!(std::time::Instant::now() < deadline, "never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
        assert_eq!(answers.faces(), None);
    }
}
