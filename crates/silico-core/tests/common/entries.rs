//! For the tests of `elements` and `query` (`silico_core::command::elements`): documents, a
//! model of one rebuilt on the fake kernel, refusals as their codes and parameters, and
//! sessions on the fake.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Problems, Response, Running, SessionId, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{Model, Recompute};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use super::model::{checked, published};

/// A 1 mm box: one body of 6 faces, 12 edges, and 8 vertices.
pub const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;

/// Two separate boxes, neither consumed: two bodies, in timeline order.
pub const TWO_BODIES: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;

/// `text` rebuilt on the fake kernel and the fake sketch solver.
pub fn model_of(text: &str) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    published(&engine, checked(text))
}

/// Each problem's code and parameter, in order.
pub fn codes(problems: &Problems) -> Vec<(ErrorCode, Option<String>)> {
    problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.parameter().map(str::to_owned)))
        .collect()
}

/// Long enough for any command here, so one that hangs fails instead.
pub const HANG: Duration = Duration::from_secs(60);

/// Sessions on a fake kernel and sketch solver, their cache sealed with a secret in `dir`.
pub struct Fixture {
    pub dir: TempDir,
    pub kernel: Arc<FakeKernel>,
    pub sessions: Sessions<Arc<FakeKernel>>,
}

pub fn fixture() -> Fixture {
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
    /// `text` written to the file `name` in the fixture's directory.
    pub fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// `text`, as the file `name`, opened in a session.
    pub fn open(&self, name: &str, text: &str) -> SessionId {
        answer(self.sessions.open(self.file(name, text), false))
            .session
            .unwrap()
            .id
    }
}

/// The command's response, failing if it doesn't answer within [`HANG`].
pub fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}
