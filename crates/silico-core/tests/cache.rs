//! M2.5: the rebuild cache. A session that opens a document another session built takes
//! each step's result from the cache, not the kernel; an edit misses only for the steps it
//! changes; another kernel or platform misses everything; an entry that doesn't hold
//! together is rebuilt and written over, never trusted; failures aren't kept on disk; and a
//! cache that can't be written to fails nothing. Expected values are written by hand: which
//! kernel calls each session makes, and the shapes, as the fake describes what made them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cache.rs"]
mod cache;
#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, session};
use example::EXAMPLE;
use model::{checked, made, published};
use silico_core::cache::Cache;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Generation, Kernel, KernelRequest, Op, Primitive};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Recompute};
use silico_core::{Error, ErrorCode};

#[test]
fn a_document_built_once_is_rebuilt_from_the_cache_with_no_kernel_call() {
    let dir = Scratch::new("hit");
    let first = session(&dir.0);
    let built = published(&first, checked(EXAMPLE));
    assert_eq!(
        calls(&first),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    // One entry per step.
    assert_eq!(files(&dir.0).len(), 4);
    drop(first);
    let second = session(&dir.0);
    let again = published(&second, checked(EXAMPLE));
    assert!(calls(&second).is_empty(), "{:?}", calls(&second));
    // Identical geometry: the same shapes, histories, and validity, feature for feature.
    assert_eq!(again.features(), built.features());
    assert_eq!(hole(&again), HOLE);
}

#[test]
fn an_edit_misses_only_for_the_steps_it_changes() {
    let dir = Scratch::new("edit");
    published(&session(&dir.0), checked(EXAMPLE));
    let second = session(&dir.0);
    let model = published(&second, example_with_pin("3"));
    // The block's make is kept; the pin's make, its move, and the cut are new.
    assert_eq!(calls(&second), ["Cut", "MakeCylinder", "Translate"]);
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
    // And both versions are now kept.
    let third = session(&dir.0);
    published(&third, checked(EXAMPLE));
    published(&third, example_with_pin("3"));
    assert!(calls(&third).is_empty());
}

/// The fake kernel under another identity: another build of the kernel.
struct Rebuilt(FakeKernel);

impl Kernel for Rebuilt {
    fn identity(&self) -> &str {
        "silico fake kernel 2"
    }

    fn generation(&self) -> Generation {
        self.0.generation()
    }

    fn advance(&self, generation: Generation) {
        self.0.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.0.submit(generation, request)
    }
}

#[test]
fn another_kernel_misses_everything() {
    let dir = Scratch::new("kernel");
    published(&session(&dir.0), checked(EXAMPLE));
    let rebuilt = Recompute::with_cache(Rebuilt(FakeKernel::new(2).unwrap()), cache::cache(&dir.0));
    published(&rebuilt, checked(EXAMPLE));
    assert_eq!(made(&rebuilt.kernel().0.calls()).len(), 4);
}

#[test]
fn another_platform_misses_everything() {
    let dir = Scratch::new("platform");
    published(&session(&dir.0), checked(EXAMPLE));
    let elsewhere = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::for_platform(&dir.0, "elsewhere-riscv64", &dir.secret()),
    );
    published(&elsewhere, checked(EXAMPLE));
    assert_eq!(
        calls(&elsewhere),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    // Both platforms' entries are kept, side by side.
    assert_eq!(files(&dir.0).len(), 8);
}

/// Damages an entry's bytes, given another entry's.
type Damage = fn(&[u8], &[u8]) -> Vec<u8>;

#[test]
fn an_entry_that_doesnt_hold_together_is_rebuilt_and_written_over() {
    // Each way an entry can be damaged: a byte flipped in its payload, in its seal, or
    // in its magic; cut short; empty; another entry's bytes under its name.
    let damages: [(&str, Damage); 6] = [
        ("payload", |bytes, _| {
            let mut bytes = bytes.to_vec();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            bytes
        }),
        ("seal", |bytes, _| {
            let mut bytes = bytes.to_vec();
            bytes[20] ^= 1;
            bytes
        }),
        ("magic", |bytes, _| {
            let mut bytes = bytes.to_vec();
            bytes[0] ^= 1;
            bytes
        }),
        ("short", |bytes, _| bytes[..bytes.len() / 2].to_vec()),
        ("empty", |_, _| Vec::new()),
        ("another", |_, other| other.to_vec()),
    ];
    for (damage, damaged) in damages {
        let dir = Scratch::new(&format!("corrupt-{damage}"));
        published(&session(&dir.0), checked(EXAMPLE));
        let files = files(&dir.0);
        let (victim, other) = (&files[0], &files[1]);
        let sound = std::fs::read(victim).unwrap();
        std::fs::write(victim, damaged(&sound, &std::fs::read(other).unwrap())).unwrap();
        let second = session(&dir.0);
        let model = published(&second, checked(EXAMPLE));
        // One step rebuilt, the rest kept; the same result.
        assert_eq!(made(&second.kernel().calls()).len(), 1, "{damage}");
        assert_eq!(hole(&model), HOLE, "{damage}");
        // Written over, sound again.
        assert_eq!(std::fs::read(victim).unwrap(), sound, "{damage}");
    }
}

#[test]
fn failures_arent_kept_on_disk() {
    let dir = Scratch::new("failure");
    let first = session(&dir.0);
    first.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    published(&first, checked(EXAMPLE));
    // The block's make only: the pin failed, and the cut was blocked.
    assert_eq!(files(&dir.0).len(), 1);
    // Another session asks the kernel again, and builds.
    let second = session(&dir.0);
    let model = published(&second, checked(EXAMPLE));
    assert_eq!(calls(&second), ["Cut", "MakeCylinder", "Translate"]);
    assert_eq!(hole(&model), HOLE);
}

#[test]
fn a_cache_that_cant_be_written_fails_nothing() {
    // A file where the cache's directory would be: nothing can be written under it.
    let dir = Scratch::new("unwritable");
    std::fs::write(&dir.0, b"not a directory").unwrap();
    let engine = session(&dir.0);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    let _ = std::fs::remove_file(&dir.0);
}

#[test]
fn a_hit_is_a_shape_the_worker_checked() {
    let dir = Scratch::new("checked");
    published(&session(&dir.0), checked(EXAMPLE));
    let model = published(&session(&dir.0), checked(EXAMPLE));
    for (_, outcome) in model.features() {
        let FeatureOutcome::Built { built, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(built.shape.valid(), Some(true));
    }
}

#[test]
fn the_cache_ignores_itself_in_any_repository() {
    // A document in a repository of its own, with no ignore rules: after a recompute, git
    // has nothing to add.
    let repo = Scratch::new("repository");
    std::fs::create_dir_all(&repo.0).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "--quiet"]);
    let dir = repo.0.join(".silico-cache");
    // The secret outside the repository, as a user's is.
    let secret = Scratch::new("repository-secret");
    let engine = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::with_secret(&dir, &secret.secret()),
    );
    published(&engine, checked(EXAMPLE));
    assert_eq!(files(&dir).len(), 4);
    assert_eq!(git(&["status", "--porcelain", "--untracked-files=all"]), "");
    // A sound one is left as it is when more entries are written: not written again.
    let ignore = || {
        let meta = std::fs::metadata(dir.join(".gitignore")).unwrap();
        (
            std::os::unix::fs::MetadataExt::ino(&meta),
            meta.modified().unwrap(),
        )
    };
    let first = ignore();
    published(&engine, example_with_pin("3"));
    assert_eq!(files(&dir).len(), 7);
    assert_eq!(ignore(), first);
}

#[test]
fn entries_are_sealed_with_their_caches_secret_which_is_made_once() {
    let dir = Scratch::new("secret");
    published(&session(&dir.0), checked(EXAMPLE));
    // Made by the first session: 32 bytes, readable by the user alone.
    let secret = std::fs::read(dir.secret()).unwrap();
    assert_eq!(secret.len(), 32);
    let mode = std::fs::metadata(dir.secret()).unwrap().permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o600
    );
    // Kept: another session reads it, and hits everything.
    let second = session(&dir.0);
    published(&second, checked(EXAMPLE));
    assert!(calls(&second).is_empty());
    assert_eq!(std::fs::read(dir.secret()).unwrap(), secret);
    // Under another secret, nothing is taken.
    let other = Scratch::new("secret-other");
    let stranger = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::with_secret(&dir.0, &other.secret()),
    );
    published(&stranger, checked(EXAMPLE));
    assert_eq!(
        calls(&stranger),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
}

#[test]
fn a_damaged_secret_keeps_nothing_and_fails_nothing() {
    for (damage, bytes) in [
        ("short", &b"too short"[..]),
        ("empty", &b""[..]),
        ("long", &[7; 33][..]),
    ] {
        let dir = Scratch::new(&format!("damaged-secret-{damage}"));
        std::fs::write(dir.secret(), bytes).unwrap();
        let first = session(&dir.0);
        assert_eq!(hole(&published(&first, checked(EXAMPLE))), HOLE, "{damage}");
        assert!(files(&dir.0).is_empty(), "{damage}");
        // Left as it was, for the user to look at: not replaced under entries sealed with it.
        assert_eq!(std::fs::read(dir.secret()).unwrap(), bytes, "{damage}");
    }
}

/// The variable that makes [`a_session_in_a_process_of_its_own`] a session over the cache
/// it names.
const AS_SESSION: &str = "SILICO_TEST_CACHE_SESSION";

#[test]
fn a_session_in_a_process_of_its_own() {
    // A session with `Cache::open`, when run by [`session_with`]; nothing otherwise.
    if let Some(dir) = std::env::var_os(AS_SESSION) {
        let engine = Recompute::with_cache(FakeKernel::new(2).unwrap(), Cache::open(dir));
        assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
    }
}

/// Runs a session with `Cache::open` over the cache in `dir`, in a process of its own (this
/// test binary, again), in `cwd`, with no `HOME` or `XDG_CACHE_HOME` but those in `env`.
fn session_with(dir: &Path, cwd: &Path, env: &[(&str, &Path)]) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "a_session_in_a_process_of_its_own",
            "--nocapture",
        ])
        .current_dir(cwd)
        .env(AS_SESSION, dir)
        .env_remove("HOME")
        .env_remove("XDG_CACHE_HOME");
    for (name, value) in env {
        command.env(name, value);
    }
    let out = command.output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("1 passed"), "{said}");
}

/// Whether a session over the cache in `dir`, with the secret in `secret`, hits every step.
fn hits_everything_with(dir: &Path, secret: &Path) -> bool {
    let engine =
        Recompute::with_cache(FakeKernel::new(2).unwrap(), Cache::with_secret(dir, secret));
    published(&engine, checked(EXAMPLE));
    calls(&engine).is_empty()
}

#[test]
fn a_session_seals_with_the_secret_in_the_users_cache_directory() {
    let dir = Scratch::new("user");
    let home = Scratch::new("user-home");
    let xdg = Scratch::new("user-xdg");
    std::fs::create_dir_all(&home.0).unwrap();
    std::fs::create_dir_all(&xdg.0).unwrap();
    session_with(
        &dir.0,
        &home.0,
        &[("HOME", &home.0), ("XDG_CACHE_HOME", &xdg.0)],
    );
    let secret = xdg.0.join("silico/cache-secret");
    assert_eq!(std::fs::read(&secret).unwrap().len(), 32);
    assert!(!home.0.join(".cache").exists());
    // The session's entries are sealed with it: a cache with that secret hits them all.
    assert!(hits_everything_with(&dir.0, &secret));
}

#[test]
fn with_no_xdg_cache_home_the_secret_is_in_the_homes_cache() {
    let dir = Scratch::new("home-only");
    let home = Scratch::new("home-only-home");
    std::fs::create_dir_all(&home.0).unwrap();
    session_with(&dir.0, &home.0, &[("HOME", &home.0)]);
    let secret = home.0.join(".cache/silico/cache-secret");
    assert_eq!(std::fs::read(&secret).unwrap().len(), 32);
    assert!(hits_everything_with(&dir.0, &secret));
}

#[test]
fn a_relative_xdg_cache_home_is_ignored() {
    // Relative to wherever the session runs, it would put the secret anywhere.
    let dir = Scratch::new("relative-xdg");
    let home = Scratch::new("relative-xdg-home");
    std::fs::create_dir_all(&home.0).unwrap();
    session_with(
        &dir.0,
        &home.0,
        &[("HOME", &home.0), ("XDG_CACHE_HOME", Path::new("relative"))],
    );
    assert!(!home.0.join("relative").exists());
    let secret = home.0.join(".cache/silico/cache-secret");
    assert!(hits_everything_with(&dir.0, &secret));
}

#[test]
fn a_relative_home_is_ignored() {
    let dir = Scratch::new("relative-home");
    let cwd = Scratch::new("relative-home-cwd");
    std::fs::create_dir_all(&cwd.0).unwrap();
    session_with(&dir.0, &cwd.0, &[("HOME", Path::new("relative"))]);
    assert!(!dir.0.exists(), "{:?}", files(&dir.0));
    assert_eq!(std::fs::read_dir(&cwd.0).unwrap().count(), 0);
}

#[test]
fn with_no_home_a_session_keeps_nothing() {
    // No secret outside the document to seal with: nothing is kept, in the cache or beside
    // it, and nothing fails.
    let dir = Scratch::new("no-home");
    let cwd = Scratch::new("no-home-cwd");
    std::fs::create_dir_all(&cwd.0).unwrap();
    session_with(&dir.0, &cwd.0, &[]);
    assert!(!dir.0.exists(), "{:?}", files(&dir.0));
    assert_eq!(std::fs::read_dir(&cwd.0).unwrap().count(), 0);
}

#[test]
fn sessions_making_the_secret_at_once_agree_on_it() {
    // Eight sessions start on a cache with no secret yet: each makes one, and all must end
    // with the one on disk, or their entries are sealed with secrets no one has.
    for round in 0..20 {
        let dir = Scratch::new(&format!("secret-race-{round}"));
        let radii = ["1", "2", "3", "4", "5", "6", "7", "8"];
        let barrier = Arc::new(Barrier::new(radii.len()));
        let threads: Vec<_> = radii
            .iter()
            .map(|radius| {
                let (dir, barrier) = (dir.0.clone(), Arc::clone(&barrier));
                let radius = (*radius).to_owned();
                std::thread::spawn(move || {
                    barrier.wait();
                    published(&session(&dir), example_with_pin(&radius));
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let last = session(&dir.0);
        for radius in radii {
            published(&last, example_with_pin(radius));
        }
        assert!(calls(&last).is_empty(), "round {round}: {:?}", calls(&last));
    }
}

#[test]
fn caches_of_two_secrets_in_one_directory_keep_each_others_entries() {
    // One document folder, synced between two machines (or used by two accounts): each
    // side's entries stay, side by side.
    let dir = Scratch::new("two-secrets");
    let theirs = Scratch::new("two-secrets-theirs");
    published(&session(&dir.0), checked(EXAMPLE));
    let other = || {
        Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&dir.0, &theirs.secret()),
        )
    };
    let first = other();
    published(&first, checked(EXAMPLE));
    assert_eq!(
        calls(&first),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert_eq!(files(&dir.0).len(), 8);
    let mine = session(&dir.0);
    published(&mine, checked(EXAMPLE));
    assert!(calls(&mine).is_empty(), "{:?}", calls(&mine));
    let again = other();
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty(), "{:?}", calls(&again));
}

#[test]
fn a_gitignore_that_isnt_the_caches_is_put_back() {
    // Empty (a write cut short), un-ignoring, or a link: each is replaced whole, and the
    // link's target is left alone.
    let elsewhere = Scratch::new("gitignore-elsewhere");
    std::fs::create_dir_all(&elsewhere.0).unwrap();
    let target = elsewhere.0.join("theirs");
    std::fs::write(&target, b"their own\n").unwrap();
    type Plant<'a> = Box<dyn Fn(&Path) + 'a>;
    let plant: [(&str, Plant); 3] = [
        ("empty", Box::new(|path| std::fs::write(path, b"").unwrap())),
        (
            "unignoring",
            Box::new(|path| std::fs::write(path, b"!*\n").unwrap()),
        ),
        (
            "link",
            Box::new(|path| std::os::unix::fs::symlink(&target, path).unwrap()),
        ),
    ];
    for (what, plant) in plant {
        let dir = Scratch::new(&format!("gitignore-{what}"));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join(".gitignore");
        plant(&path);
        published(&session(&dir.0), checked(EXAMPLE));
        assert!(!path.is_symlink(), "{what}");
        let ignore = std::fs::read_to_string(&path).unwrap();
        assert_eq!(ignore.lines().last(), Some("*"), "{what}: {ignore:?}");
        assert!(
            ignore
                .lines()
                .all(|line| line == "*" || line.starts_with('#')),
            "{what}: {ignore:?}"
        );
    }
    assert_eq!(std::fs::read(&target).unwrap(), b"their own\n");
}

#[test]
fn a_cache_whose_directory_is_a_link_keeps_nothing_where_it_points() {
    // A `.silico-cache` that links elsewhere (one that came with a document): into the
    // repository, entries there would be committed, git keeping only the link's
    // `.gitignore`-less target; elsewhere, the directory may be anyone's. Nothing is
    // written, and nothing fails.
    let base = Scratch::new("linked-root");
    std::fs::create_dir_all(base.0.join("target")).unwrap();
    let link = base.0.join(".silico-cache");
    std::os::unix::fs::symlink("target", &link).unwrap();
    let secret = Scratch::new("linked-root-secret");
    for _ in 0..2 {
        let engine = Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&link, &secret.secret()),
        );
        assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
        assert_eq!(
            calls(&engine),
            ["Cut", "MakeBox", "MakeCylinder", "Translate"]
        );
    }
    assert_eq!(std::fs::read_dir(base.0.join("target")).unwrap().count(), 0);
}

#[test]
fn a_linked_cache_takes_nothing_kept_where_it_points() {
    // Entries of this user's, kept in a directory a link then points at: not read
    // through the link either.
    let base = Scratch::new("linked-read");
    let target = base.0.join("target");
    let secret = Scratch::new("linked-read-secret");
    published(
        &Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&target, &secret.secret()),
        ),
        checked(EXAMPLE),
    );
    assert_eq!(files(&target).len(), 4);
    let link = base.0.join(".silico-cache");
    std::os::unix::fs::symlink("target", &link).unwrap();
    let engine = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::with_secret(&link, &secret.secret()),
    );
    assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
    assert_eq!(
        calls(&engine),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
}

#[test]
fn a_linked_cache_named_with_a_trailing_slash_keeps_nothing() {
    // `link/` and `link/.` resolve the link: named so, it must still be seen as one.
    for (n, name) in [".silico-cache/", ".silico-cache/."]
        .into_iter()
        .enumerate()
    {
        let base = Scratch::new(&format!("linked-slash-{n}"));
        std::fs::create_dir_all(base.0.join("target")).unwrap();
        std::os::unix::fs::symlink("target", base.0.join(".silico-cache")).unwrap();
        let secret = Scratch::new(&format!("linked-slash-{n}-secret"));
        let engine = Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(base.0.join(name), &secret.secret()),
        );
        assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE, "{name}");
        let written = std::fs::read_dir(base.0.join("target")).unwrap().count();
        assert_eq!(written, 0, "{name}");
    }
}

#[test]
fn a_cache_takes_nothing_kept_behind_subdirectories_that_are_links() {
    // This user's entries, in a cache elsewhere; a cache whose subdirectories link to that
    // one's reads none of them.
    let base = Scratch::new("linked-shards-read");
    let kept = base.0.join("kept");
    let secret = Scratch::new("linked-shards-read-secret");
    published(
        &Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&kept, &secret.secret()),
        ),
        checked(EXAMPLE),
    );
    let root = base.0.join(".silico-cache");
    std::fs::create_dir_all(&root).unwrap();
    for sub in std::fs::read_dir(&kept).unwrap().flatten() {
        if sub.path().is_dir() {
            std::os::unix::fs::symlink(
                Path::new("../kept").join(sub.file_name()),
                root.join(sub.file_name()),
            )
            .unwrap();
        }
    }
    let engine = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::with_secret(&root, &secret.secret()),
    );
    assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
    assert_eq!(
        calls(&engine),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
    assert_eq!(files(&kept).len(), 4);
}

#[test]
fn a_cache_whose_subdirectories_are_links_keeps_nothing_where_they_point() {
    // A real `.silico-cache` with a link in place of every subdirectory (force-added to a
    // repository: git keeps tracked links whatever `.gitignore` says).
    let base = Scratch::new("linked-shards");
    let target = base.0.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("theirs"), b"their own").unwrap();
    let root = base.0.join(".silico-cache");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..256 {
        std::os::unix::fs::symlink("../target", root.join(format!("{i:02x}"))).unwrap();
    }
    let secret = Scratch::new("linked-shards-secret");
    for _ in 0..2 {
        let engine = Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&root, &secret.secret()),
        );
        assert_eq!(hole(&published(&engine, checked(EXAMPLE))), HOLE);
        assert_eq!(
            calls(&engine),
            ["Cut", "MakeBox", "MakeCylinder", "Translate"]
        );
    }
    let written: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(written, ["theirs"], "written through the links");
}
