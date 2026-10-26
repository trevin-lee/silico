//! The rebuild cache in a workspace (decision 26 of docs/design/m4.1-command-api.md;
//! `Cache::within`): every read, write, eviction, and clear checks, as it begins, that the
//! cache's directory still leads nowhere out of the workspace, so a link put on its way after
//! the cache was opened (its document's directory, a directory above it, or `.silico-cache`
//! itself, in a transaction, which reads nothing again) has nothing read, written, evicted,
//! or removed through it: a read misses, the rest do nothing, and the rebuild is still right.
//! Without a workspace (the CLI), a cache through a link is used as it always was. Expected
//! values are by hand: the files each test planted, which kernel calls a rebuild makes, and
//! the shapes as the fake describes what made them.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use silico_core::cache::Cache;
use silico_core::command::{CheckModel, SessionId};
use silico_core::fake::FakeKernel;
use silico_core::file::Workspace;
use silico_core::recompute::Recompute;

use crate::cache::{HOLE, calls, hole, subdirectory_files};
use crate::cache_control::{NAME, entries};
use crate::calls::{BOX, Fixture, fixture};
use crate::example::EXAMPLE;
use crate::model::{checked, made, published};
use crate::sessions::answer;

/// `dir` moved to `to`, and a link to it put in its place.
fn swap_out(dir: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::rename(dir, to).unwrap();
    std::os::unix::fs::symlink(to, dir).unwrap();
}

/// How many boxes the fixture's kernel has made.
fn boxes_made(f: &Fixture) -> usize {
    made(&f.kernel.calls())
        .into_iter()
        .filter(|call| *call == "MakeBox")
        .count()
}

/// The box at `document` (in the workspace) opened and built, its cache's directory removed
/// so nothing is on disk, and a transaction begun; then `swap` run, and a fresh check, which
/// must build the box again, with no problems. The session.
fn rebuilt_in_a_transaction_after(f: &Fixture, document: &str, swap: impl Fn()) -> SessionId {
    f.file(document, BOX);
    let open = answer(f.sessions.open(document, false));
    let s = open.session.unwrap().id;
    let fresh = CheckModel::Current { fresh: true };
    assert!(
        answer(f.sessions.check(&s, fresh))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let cache = f.work.join(document).with_file_name(".silico-cache");
    std::fs::remove_dir_all(&cache).unwrap();
    assert!(answer(f.sessions.begin(&s)).result.is_ok());
    swap();
    let before = boxes_made(f);
    let check = answer(f.sessions.check(&s, fresh));
    assert!(check.result.unwrap().problems.is_empty());
    // Built again, by the kernel.
    assert_eq!(boxes_made(f), before + 1);
    s
}

#[test]
fn a_rebuild_after_its_documents_directory_is_swapped_for_a_link_out_writes_nothing_outside() {
    // The case #183's review proved: `begin`, the document's directory swapped for a link
    // out, and a fresh check, which left an entry in `outside/sub/.silico-cache/`.
    let f = fixture();
    let moved = f.outside.join("sub");
    let s = rebuilt_in_a_transaction_after(&f, "sub/part.silico", || {
        swap_out(&f.work.join("sub"), &moved);
    });
    assert!(moved.join("part.silico").exists());
    assert!(!moved.join(".silico-cache").exists());
    // And `cache_clear` says it removed nothing, the directory named as the session's path
    // names it.
    let cleared = answer(f.sessions.cache_clear(&s)).result.unwrap();
    assert_eq!(
        (cleared.directory.as_str(), cleared.removed),
        ("sub/.silico-cache", 0)
    );
}

#[test]
fn a_rebuild_after_a_directory_above_its_document_is_swapped_for_a_link_out_writes_none_out() {
    let f = fixture();
    let moved = f.outside.join("a");
    let s = rebuilt_in_a_transaction_after(&f, "a/sub/part.silico", || {
        swap_out(&f.work.join("a"), &moved);
    });
    assert!(moved.join("sub/part.silico").exists());
    assert!(!moved.join("sub/.silico-cache").exists());
    let cleared = answer(f.sessions.cache_clear(&s)).result.unwrap();
    assert_eq!(
        (cleared.directory.as_str(), cleared.removed),
        ("a/sub/.silico-cache", 0)
    );
}

#[test]
fn a_rebuild_after_its_cache_directory_is_swapped_for_a_link_out_writes_nothing_outside() {
    let f = fixture();
    let theirs = f.outside.join("cache");
    std::fs::create_dir(&theirs).unwrap();
    let s = rebuilt_in_a_transaction_after(&f, "sub/part.silico", || {
        std::os::unix::fs::symlink(&theirs, f.work.join("sub/.silico-cache")).unwrap();
    });
    assert_eq!(std::fs::read_dir(&theirs).unwrap().count(), 0);
    let cleared = answer(f.sessions.cache_clear(&s)).result.unwrap();
    assert_eq!(
        (cleared.directory.as_str(), cleared.removed),
        ("sub/.silico-cache", 0)
    );
}

/// A scratch root (through its links), holding a workspace, `work`, with `work/sub` in it,
/// and a directory beside it, `outside`.
struct Scene {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    outside: PathBuf,
}

impl Scene {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (work, outside) = (root.join("work"), root.join("outside"));
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        Scene {
            _dir: dir,
            root,
            work,
            outside,
        }
    }

    /// The cache beside `work/sub`'s documents, its secret outside both directories.
    fn cache(&self) -> Cache {
        Cache::with_secret(
            self.work.join("sub/.silico-cache"),
            &self.root.join("secret"),
        )
    }

    /// [`cache`](Self::cache), in the workspace.
    fn within(&self) -> Cache {
        self.cache().within(Workspace::new(&self.work).unwrap())
    }

    /// `work/sub` moved outside, and a link to it put in its place: its cache, there.
    fn swap(&self) -> PathBuf {
        swap_out(&self.work.join("sub"), &self.outside.join("sub"));
        self.outside.join("sub/.silico-cache")
    }
}

/// An engine on a new fake kernel, over `cache`.
fn engine(cache: Cache) -> Recompute<FakeKernel> {
    Recompute::with_cache(FakeKernel::new(2).unwrap(), cache)
}

/// Each file at `paths`, and when it was last modified.
fn modified(paths: &[PathBuf]) -> Vec<(PathBuf, SystemTime)> {
    paths
        .iter()
        .map(|path| {
            let time = std::fs::metadata(path).unwrap().modified().unwrap();
            (path.clone(), time)
        })
        .collect()
}

/// Marks the file at `path` last modified `ago`.
fn age(path: &Path, ago: Duration) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - ago).unwrap();
}

#[test]
fn a_read_after_its_directory_leads_out_misses_and_takes_nothing_from_outside() {
    // The example built in the workspace, its four entries kept, sound and this secret's;
    // then its directory moved outside, a link in its place: a second engine over the
    // cache, in the workspace, takes none of them (every step made by the kernel), touches
    // none (their times as they were), and writes none beside them.
    let scene = Scene::new();
    published(&engine(scene.within()), checked(EXAMPLE));
    let outside = scene.swap();
    let kept = entries(&outside);
    assert_eq!(kept.len(), 4);
    for entry in &kept {
        age(entry, Duration::from_secs(24 * 60 * 60));
    }
    let times = modified(&kept);
    let second = engine(scene.within());
    let model = published(&second, checked(EXAMPLE));
    assert_eq!(
        calls(&second),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert_eq!(hole(&model), HOLE);
    assert_eq!(subdirectory_files(&outside), kept);
    assert_eq!(modified(&kept), times);
}

#[test]
fn a_put_after_its_directory_leads_out_evicts_and_removes_nothing_outside() {
    // Outside, at the cache's directory through the link: an entry of the whole bound,
    // used a day ago, and a partial file two hours stale. A put through the link would scan
    // (its first), remove the partial, and evict the entry to make room; in the workspace,
    // it does nothing, and both are left, with nothing beside them.
    let scene = Scene::new();
    let outside = scene.swap();
    let bound = 1 << 20;
    let sub = outside.join("ab");
    std::fs::create_dir_all(&sub).unwrap();
    let entry = sub.join(NAME);
    std::fs::File::create(&entry)
        .unwrap()
        .set_len(bound)
        .unwrap();
    age(&entry, Duration::from_secs(24 * 60 * 60));
    let partial = outside.join(".partial-1-1");
    std::fs::write(&partial, b"theirs").unwrap();
    age(&partial, Duration::from_secs(2 * 60 * 60));
    let engine = engine(scene.within().with_bound(bound));
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    assert_eq!(
        calls(&engine),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert_eq!(std::fs::metadata(&entry).unwrap().len(), bound);
    assert_eq!(std::fs::read(&partial).unwrap(), b"theirs");
    assert_eq!(subdirectory_files(&outside), [entry]);
}

#[test]
fn a_clear_after_its_directory_leads_out_removes_nothing_outside() {
    // Two files named as entries, outside, where the cache's directory leads through the
    // link: the clear removes neither, and says it removed none; put back inside, the same
    // cache clears both.
    let scene = Scene::new();
    let outside = scene.swap();
    let planted: Vec<PathBuf> = ["ab", "cd"]
        .into_iter()
        .map(|sub| {
            std::fs::create_dir_all(outside.join(sub)).unwrap();
            let entry = outside.join(sub).join(NAME);
            std::fs::write(&entry, b"theirs").unwrap();
            entry
        })
        .collect();
    let cache = scene.within();
    assert_eq!(cache.clear().unwrap(), 0);
    for entry in &planted {
        assert_eq!(std::fs::read(entry).unwrap(), b"theirs", "{entry:?}");
    }
    std::fs::remove_file(scene.work.join("sub")).unwrap();
    std::fs::rename(scene.outside.join("sub"), scene.work.join("sub")).unwrap();
    assert_eq!(cache.clear().unwrap(), 2);
    assert!(entries(&scene.work.join("sub/.silico-cache")).is_empty());
}

#[test]
fn without_a_workspace_a_cache_through_a_link_is_used_as_before() {
    // The CLI's: no workspace, so the cache's directory is used wherever its links lead.
    // Built through the link, its four entries are outside; a second engine takes all of
    // them, with no kernel call; a clear removes them.
    let scene = Scene::new();
    let outside = scene.swap();
    published(&engine(scene.cache()), checked(EXAMPLE));
    assert_eq!(entries(&outside).len(), 4);
    let second = engine(scene.cache());
    let model = published(&second, checked(EXAMPLE));
    assert!(calls(&second).is_empty(), "{:?}", calls(&second));
    assert_eq!(hole(&model), HOLE);
    assert_eq!(scene.cache().clear().unwrap(), 4);
    assert!(entries(&outside).is_empty());
}
