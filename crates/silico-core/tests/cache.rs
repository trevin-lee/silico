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

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, platform, session};
use example::EXAMPLE;
use model::{checked, published};
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
    assert_eq!(rebuilt.kernel().0.calls().len(), 4);
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
        assert_eq!(second.kernel().calls().len(), 1, "{damage}");
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
        Cache::for_platform(&dir, &platform(), &secret.secret()),
    );
    published(&engine, checked(EXAMPLE));
    assert_eq!(files(&dir).len(), 4);
    assert_eq!(git(&["status", "--porcelain", "--untracked-files=all"]), "");
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
        Cache::for_platform(&dir.0, &platform(), &other.secret()),
    );
    published(&stranger, checked(EXAMPLE));
    assert_eq!(
        calls(&stranger),
        ["Cut", "MakeBox", "MakeCylinder", "Translate"]
    );
}

#[test]
fn a_damaged_secret_keeps_nothing_and_fails_nothing() {
    for (damage, bytes) in [("short", &b"too short"[..]), ("empty", &b""[..])] {
        let dir = Scratch::new(&format!("damaged-secret-{damage}"));
        std::fs::write(dir.secret(), bytes).unwrap();
        let first = session(&dir.0);
        assert_eq!(hole(&published(&first, checked(EXAMPLE))), HOLE, "{damage}");
        assert!(files(&dir.0).is_empty(), "{damage}");
        // Left as it was, for the user to look at: not replaced under entries sealed with it.
        assert_eq!(std::fs::read(dir.secret()).unwrap(), bytes, "{damage}");
    }
}

/// The variable that makes [`a_session_seals_with_the_users_secret`] the session itself.
const AS_SESSION: &str = "SILICO_TEST_CACHE_SESSION";

#[test]
fn a_session_seals_with_the_users_secret() {
    // `Cache::open` in a process of its own (this test, again), with the user's cache
    // directory set: its secret is `silico/cache-secret` there.
    if let Some(dir) = std::env::var_os(AS_SESSION) {
        let engine = Recompute::with_cache(FakeKernel::new(2).unwrap(), Cache::open(dir));
        published(&engine, checked(EXAMPLE));
        return;
    }
    let dir = Scratch::new("user");
    let home = Scratch::new("user-home");
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_session_seals_with_the_users_secret",
            "--nocapture",
        ])
        .env(AS_SESSION, &dir.0)
        .env("XDG_CACHE_HOME", &home.0)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let secret: &Path = &home.0.join("silico/cache-secret");
    assert_eq!(std::fs::read(secret).unwrap().len(), 32);
    // The session's entries are sealed with it: a cache with that secret hits them all.
    let again = Recompute::with_cache(
        FakeKernel::new(2).unwrap(),
        Cache::for_platform(&dir.0, &platform(), secret),
    );
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty());
}
