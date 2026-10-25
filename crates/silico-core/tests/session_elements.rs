//! M4.4: `elements` and `query` on a session (docs/design/m4.1-command-api.md, section 7):
//! of the current model, waited for, or the newest published one (decision 33), each answer
//! saying which model it read, and a published read asking the kernel nothing, its solids
//! described as they were rebuilt. On the fake kernel, which describes each element as its
//! number; the entries themselves are tested in tests/entries.rs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::elements::{MeasureName, Order, Query};
use silico_core::command::{ModelChoice, ModelRead, Response, Revision, Running, Sessions};
use silico_core::document::FeatureId;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::ElementKind;
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

const HANG: Duration = Duration::from_secs(60);

struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    Fixture {
        dir,
        kernel,
        sessions,
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

fn faces(limit: u32) -> Query {
    Query {
        feature: None,
        kind: ElementKind::Face,
        r#type: None,
        stable: None,
        facing: None,
        measures: Vec::new(),
        order: None,
        limit,
    }
}

#[test]
fn elements_of_the_current_model_wait_for_its_rebuild_and_say_which_model_they_read() {
    let f = fixture();
    let opened = answer(f.sessions.open(f.file("box.silico", BOX), false));
    let s = opened.session.unwrap().id;
    let listed = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 1000),
    );
    assert_eq!(
        listed.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current
        })
    );
    let listed = listed.result.unwrap();
    // A box: 6 faces, 12 edges, 8 vertices.
    assert_eq!(listed.total, 26);
    assert!(listed.next.is_none());
    // Of one feature, by its ID, in pages: the cursor holds the revision read.
    let page = answer(f.sessions.elements(
        &s,
        ModelChoice::Current,
        Some(FeatureId::new("k7f2qx3m").unwrap()),
        None,
        10,
    ))
    .result
    .unwrap();
    assert_eq!(page.entries.len(), 10);
    let next = page.next.unwrap();
    assert_eq!(next.revision, Revision::OPENED);
    let rest = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, Some(next), 1000),
    )
    .result
    .unwrap();
    assert_eq!(rest.entries.len(), 16);
    assert_eq!(rest.entries, listed.entries[10..]);
}

#[test]
fn a_published_read_asks_the_kernel_nothing_its_solids_described_as_they_were_rebuilt() {
    let f = fixture();
    let s = answer(f.sessions.open(f.file("box.silico", BOX), false))
        .session
        .unwrap()
        .id;
    // Nothing published yet: refused, naming model.
    let none = answer(
        f.sessions
            .elements(&s, ModelChoice::Published, None, None, 10),
    );
    let problems = none.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("model"));
    // Once the current model is had, the published read lists it, and asks nothing more.
    let current = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 1000),
    )
    .result
    .unwrap();
    let asked = f.kernel.calls().len();
    let published = answer(
        f.sessions
            .elements(&s, ModelChoice::Published, None, None, 1000),
    );
    assert_eq!(
        published.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published
        })
    );
    assert_eq!(published.result.unwrap().entries, current.entries);
    let queried = answer(f.sessions.query(&s, ModelChoice::Published, faces(2)));
    assert_eq!(queried.result.unwrap().matched, 6);
    assert_eq!(f.kernel.calls().len(), asked);
}

#[test]
fn a_query_on_a_session_ties_as_on_a_model_and_a_bad_one_is_refused_in_the_response() {
    let f = fixture();
    let s = answer(f.sessions.open(f.file("box.silico", BOX), false))
        .session
        .unwrap()
        .id;
    // Every face of area 1 on the fake: the largest is a tie of all six, returned whole.
    let mut largest = faces(1);
    largest.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = answer(f.sessions.query(&s, ModelChoice::Current, largest))
        .result
        .unwrap();
    assert_eq!(got.entries.len(), 6);
    let tied = got.tied.unwrap();
    assert_eq!((tied.count, tied.included), (6, true));
    // A limit of 0 is refused, the session still answering.
    let refused = answer(f.sessions.query(&s, ModelChoice::Current, faces(0)));
    assert_eq!(refused.session.map(|info| info.id), Some(s));
    let problems = refused.result.unwrap_err();
    assert_eq!(
        (
            problems.errors()[0].code(),
            problems.errors()[0].parameter()
        ),
        (ErrorCode::InvalidInput, Some("limit"))
    );
}
