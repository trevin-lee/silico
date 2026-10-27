//! The rebuild cache in a workspace (decision 26 of docs/design/m4.1-command-api.md;
//! `Cache::within`): every read, write, eviction, and clear checks, as it opens the cache's
//! directory, that it still leads nowhere out of the workspace, so a link put on its way after
//! the cache was opened (its document's directory, a directory above it, or `.silico-cache`
//! itself, in a transaction, which reads nothing again) has nothing read, written, evicted,
//! or removed through it: a read misses, the rest do nothing, and the rebuild is still right.
//! Without a workspace (the CLI), a cache through a link is used as it always was. A swap
//! made while a read, write, or clear has the directory open redirects nothing: each does
//! all it does through the handle it opened. Expected values are by hand: the files each
//! test planted, which kernel calls a rebuild makes, and the shapes as the fake describes
//! what made them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

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

/// Two files named as entries in `dir`, `ab/NAME` and `cd/NAME`, each holding `bytes`.
fn plant(dir: &Path, bytes: &[u8]) -> Vec<PathBuf> {
    ["ab", "cd"]
        .into_iter()
        .map(|sub| {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            let entry = dir.join(sub).join(NAME);
            std::fs::write(&entry, bytes).unwrap();
            entry
        })
        .collect()
}

#[test]
fn a_clear_after_its_directory_leads_out_removes_nothing_outside() {
    // Two files named as entries, outside, where the cache's directory leads through the
    // link: the clear removes neither, and says it removed none; put back inside, the same
    // cache clears both.
    let scene = Scene::new();
    let outside = scene.swap();
    let planted = plant(&outside, b"theirs");
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

/// A log layer that runs its swap once, as the first span named `span` opens, and says
/// whether it has.
struct SwapAt {
    span: &'static str,
    swap: Box<dyn Fn() + Send + Sync>,
    done: Arc<AtomicBool>,
}

impl SwapAt {
    fn new(span: &'static str, swap: impl Fn() + Send + Sync + 'static) -> Self {
        SwapAt {
            span,
            swap: Box::new(swap),
            done: Arc::default(),
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for SwapAt {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == self.span && !self.done.swap(true, Ordering::SeqCst) {
            (self.swap)();
        }
    }
}

#[test]
fn a_put_whose_documents_directory_is_swapped_out_as_it_evicts_evicts_and_writes_none_out() {
    // Found by #188's review: a put checked its directory once, then scanned, evicted, and
    // wrote by path, so its document's directory swapped for a link out during the scan
    // had it evict outside and write its entry there. Outside, where the link leads: an
    // entry of the whole bound, used a day ago, and a partial file two hours stale, which
    // a scan through the link would evict and remove. The box's one step is put; as its
    // eviction opens its span (`cache.evict`), `work/sub` is moved aside, inside, and a link
    // to `outside/sub` put in its place. Both files outside are left as they were, nothing
    // beside them, and the box's entry is written in the directory the put opened, now
    // `work/sub-was/.silico-cache`. The put runs on the recompute's thread, so the layer is
    // the process's: nextest runs each test in a process of its own.
    let scene = Scene::new();
    let theirs = scene.outside.join("sub/.silico-cache");
    let bound = 1 << 20;
    std::fs::create_dir_all(theirs.join("ab")).unwrap();
    let entry = theirs.join("ab").join(NAME);
    std::fs::File::create(&entry)
        .unwrap()
        .set_len(bound)
        .unwrap();
    age(&entry, Duration::from_secs(24 * 60 * 60));
    let partial = theirs.join(".partial-1-1");
    std::fs::write(&partial, b"theirs").unwrap();
    age(&partial, Duration::from_secs(2 * 60 * 60));
    let (work, outside) = (scene.work.clone(), scene.outside.clone());
    let moved = scene.work.join("sub-was");
    let aside = moved.clone();
    let layer = SwapAt::new("cache.evict", move || {
        std::fs::rename(work.join("sub"), &aside).unwrap();
        std::os::unix::fs::symlink(outside.join("sub"), work.join("sub")).unwrap();
    });
    let swapped = Arc::clone(&layer.done);
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer)).unwrap();
    let engine = engine(scene.within().with_bound(bound));
    published(&engine, checked(BOX));
    assert_eq!(calls(&engine), ["MakeBox"]);
    assert!(swapped.load(Ordering::SeqCst));
    assert_eq!(std::fs::metadata(&entry).unwrap().len(), bound);
    assert_eq!(std::fs::read(&partial).unwrap(), b"theirs");
    let mut names: Vec<_> = std::fs::read_dir(&theirs)
        .unwrap()
        .map(|item| item.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, [".partial-1-1", "ab"]);
    assert_eq!(subdirectory_files(&theirs), [entry]);
    assert_eq!(entries(&moved.join(".silico-cache")).len(), 1);
}

#[test]
fn a_clear_whose_directory_is_swapped_for_a_link_out_as_it_walks_removes_only_its_own() {
    // Found by #188's review: a clear checked its directory once, then walked it by path, so
    // `.silico-cache` swapped mid-walk for a link to another directory had it remove all
    // that's named as entries there, as a SHA-256 git repository's loose objects are (two
    // hex digits, then 62). The cache's own two entries, `ab/NAME` and `cd/NAME`; two named
    // alike outside, in `objects`; and, as the walk enters its first subdirectory (its span,
    // `cache.walk`, on this thread), `.silico-cache` moved aside and a link to `objects` put
    // in its place. The clear goes on in the directory it opened: its own two removed, and
    // counted, and those outside left.
    let scene = Scene::new();
    let ours = scene.work.join("sub/.silico-cache");
    plant(&ours, b"ours");
    let objects = scene.outside.join("objects");
    let theirs = plant(&objects, b"theirs");
    let moved = scene.work.join("sub/.silico-cache-was");
    let (link, aside, target) = (ours.clone(), moved.clone(), objects.clone());
    let layer = SwapAt::new("cache.walk", move || {
        std::fs::rename(&link, &aside).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
    });
    let swapped = Arc::clone(&layer.done);
    let cache = scene.within();
    let cleared =
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            cache.clear()
        });
    assert!(swapped.load(Ordering::SeqCst));
    assert_eq!(cleared.unwrap(), 2);
    for entry in &theirs {
        assert_eq!(std::fs::read(entry).unwrap(), b"theirs", "{entry:?}");
    }
    assert!(entries(&moved).is_empty(), "{:?}", entries(&moved));
    assert!(std::fs::symlink_metadata(&ours).unwrap().is_symlink());
}

#[test]
fn a_clear_raced_by_a_directory_flipping_with_a_link_out_removes_nothing_outside() {
    // Found by #188's second review: the clear opened `work/sub/.silico-cache` (only its own
    // name not followed), then resolved the path and compared the two by device and inode.
    // With `work/sub` exchanged, again and again, with `work/link` (a link to `outside/x`),
    // the open could go through the link, and the resolve and the stat through the
    // directory and then the link again, so the handle on `outside/x/.silico-cache` passed,
    // and the clear removed another cache's entry there. A race, so a loop: clears, as
    // another thread flips the two at uneven intervals, until the entry outside is gone
    // (the bug, found within seconds each run before the fix), 300,000 clears, or 20
    // seconds, whichever is first. A pass is no proof, but a failure is one.
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    let scene = Scene::new();
    let (work, outside) = (scene.work.clone(), scene.outside.clone());
    std::fs::create_dir_all(work.join("sub/.silico-cache")).unwrap();
    let theirs = outside.join("x/.silico-cache");
    std::fs::create_dir_all(theirs.join("ab")).unwrap();
    let entry = theirs.join("ab").join(NAME);
    std::fs::write(&entry, b"theirs").unwrap();
    std::os::unix::fs::symlink(outside.join("x"), work.join("link")).unwrap();
    let cache = scene.within();
    let stop = Arc::new(AtomicBool::new(false));
    let flipper = {
        let (stop, work) = (Arc::clone(&stop), work.clone());
        std::thread::spawn(move || {
            let mut k: u64 = 0;
            while !stop.load(Ordering::Relaxed) {
                renameat_with(
                    CWD,
                    work.join("sub"),
                    CWD,
                    work.join("link"),
                    RenameFlags::EXCHANGE,
                )
                .unwrap();
                k = (k + 7919) % 4001;
                for _ in 0..k {
                    std::hint::spin_loop();
                }
            }
        })
    };
    let began = std::time::Instant::now();
    let mut removed_at = None;
    for i in 0..300_000 {
        let _ = cache.clear();
        if !entry.exists() {
            removed_at = Some(i);
            break;
        }
        if began.elapsed() > Duration::from_secs(20) {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    flipper.join().unwrap();
    assert_eq!(
        removed_at, None,
        "a clear removed an entry outside the workspace"
    );
}
