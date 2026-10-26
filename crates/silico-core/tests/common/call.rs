//! For tests of commands called by name (`Sessions::call`) and of sessions in a workspace:
//! the documents they open, a scratch directory holding the workspace and a directory beside
//! it, sessions on a fake kernel and a fake renderer in it, and waiting for a call's answer,
//! failing if it hangs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Calling, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};
use tempfile::TempDir;

use crate::sessions::HANG;

/// A 40 × 20 × 10 box.
pub const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// Another box, 30 long, named `Theirs`: the file outside the workspace.
pub const OUTSIDE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Theirs", "position": [0, 0, 0], "length": 30, "width": 20, "height": 10}]}"#;

/// The most an error's envelope may reasonably be: every value quoted in a message is cut
/// to a few dozen characters (`json::shown`), and a path to the most the system takes, so a
/// refusal is some hundreds of bytes.
pub const REFUSAL_BOUND: usize = 16 << 10;

/// The cache directories a fixture's sessions opened, in order.
pub type Opened = Arc<Mutex<Vec<PathBuf>>>;

/// A scratch directory, `root` (through its links, as a workspace's root is: macOS's
/// temporary directory is a link), holding the workspace, `work`, and a directory beside it,
/// `outside`, which holds `part.silico` ([`OUTSIDE`]); sessions on a fake kernel, drawing
/// with a fake renderer, in the workspace, each cache's secret outside both, and the cache
/// directories they opened, in order.
pub struct Fixture {
    pub _dir: TempDir,
    pub root: PathBuf,
    pub work: PathBuf,
    pub outside: PathBuf,
    pub kernel: Arc<FakeKernel>,
    pub sessions: Sessions<Arc<FakeKernel>>,
    pub caches: Opened,
}

/// Sessions on a fake kernel, drawing with a fake renderer, with no workspace (the CLI's),
/// each cache's secret in `dir`, and the cache directories they open, in order; `before`
/// called with each cache directory as it's opened, first.
pub fn sessions_on(
    dir: &Path,
    before: impl Fn(&Path) + Send + Sync + 'static,
) -> (Arc<FakeKernel>, Sessions<Arc<FakeKernel>>, Opened) {
    let secret = dir.join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let caches = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&caches);
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        before(cache);
        seen.lock().unwrap().push(cache.to_owned());
        Cache::with_secret(cache, &secret)
    })
    .rendering(FakeRenderer::new());
    (kernel, sessions, caches)
}

/// [`sessions_on`] `scratch`, in the workspace `workspace`.
pub fn sessions_in(scratch: &Path, workspace: &Path) -> Sessions<Arc<FakeKernel>> {
    let (_, sessions, _) = sessions_on(scratch, |_| {});
    sessions.in_workspace(workspace).unwrap()
}

pub fn fixture() -> Fixture {
    fixture_with(|_| {})
}

/// A [`fixture`] whose sessions call `before` with each cache directory as they open it,
/// first: to hold an open there, past every check of its file and before it's recorded open.
pub fn fixture_with(before: impl Fn(&Path) + Send + Sync + 'static) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let work = root.join("work");
    let outside = root.join("outside");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("part.silico"), OUTSIDE).unwrap();
    let (kernel, sessions, caches) = sessions_on(&root, before);
    let sessions = sessions.in_workspace(&work).unwrap();
    Fixture {
        _dir: dir,
        root,
        work,
        outside,
        kernel,
        sessions,
        caches,
    }
}

impl Fixture {
    /// `text` written at `name` in the workspace, its directories made.
    pub fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.work.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The command `name` called on `arguments`, which must be a command's.
    pub fn calling(&self, name: &str, arguments: &str) -> Calling {
        calling(&self.sessions, name, arguments)
    }

    /// The answer of the command `name` called on `arguments`.
    pub fn call(&self, name: &str, arguments: &str) -> Answered {
        answered(self.calling(name, arguments))
    }

    /// The answer of `open` called on `path`.
    pub fn open(&self, path: &str, create: bool) -> Answered {
        self.call("open", &open_request(path, create))
    }

    /// `part.silico` (the box) written and opened as s1.
    pub fn opened(&self) {
        self.file("part.silico", BOX);
        let open = self.open("part.silico", false);
        assert!(open.ok, "{}", open.text);
    }

    /// The outside file's bytes, which no test may change.
    pub fn theirs(&self) -> String {
        std::fs::read_to_string(self.outside.join("part.silico")).unwrap()
    }
}

/// `open`'s request for `path`.
pub fn open_request(path: &str, create: bool) -> String {
    format!(
        r#"{{"path": {}, "create": {create}}}"#,
        serde_json::to_string(path).unwrap()
    )
}

/// The command `name` called on `sessions` with `arguments`, which must be a command's.
pub fn calling(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Calling {
    match sessions.call(name, arguments) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("{name} is no command"),
    }
}

/// The answer of the command `name` called on `sessions` with `arguments`.
pub fn call(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Answered {
    answered(calling(sessions, name, arguments))
}

/// The call's answer, failing if it hangs.
pub fn answered(calling: Calling) -> Answered {
    match calling.wait_timeout(HANG) {
        Ok(answered) => answered,
        Err(_) => panic!("the call didn't answer in {HANG:?}"),
    }
}

/// The answer's envelope, as a value.
pub fn envelope(answered: &Answered) -> serde_json::Value {
    serde_json::from_str(&answered.text).unwrap()
}
