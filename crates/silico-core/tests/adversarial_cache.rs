//! break-it: the M2.5 rebuild cache under hostile disks. Entries replaced by FIFOs,
//! directories, unreadable files, forged bytes with a sound seal, and entries planted under
//! another's seal; symlinks planted
//! where the cache writes its partial files; a cache directory removed mid-run, made
//! read-only, or with a file where a subdirectory goes; sessions racing on one cache; and
//! keys whose parts run together. Expected values are written by hand: which kernel calls a
//! session makes, and the fake's description of each shape.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cache.rs"]
mod cache;
#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, platform, secret_of, session};
use example::EXAMPLE;
use hmac::{Hmac, Mac};
use model::{checked, published};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use silico_core::cache::Cache;
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Generation, History, InputHistory, Kernel, KernelRequest};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

const MAGIC: &[u8] = b"silico cache 1\n";

fn built(model: &Model, id: &str) -> (String, Arc<silico_core::recompute::Built>) {
    match model.outcome(&FeatureId::new(id).unwrap()) {
        Some(FeatureOutcome::Built { key, built }) => (key.to_string(), Arc::clone(built)),
        other => panic!("{id}: {other:?}"),
    }
}

/// Where the fake kernel's entry for step `key` lives, derived by hand from the format the
/// module documents: SHA-256 of the magic, then the kernel, the platform, and the step,
/// each after its length (8 bytes, little-endian); the first two hex digits are the
/// subdirectory.
fn entry_path(dir: &Path, step: &str) -> PathBuf {
    let platform = platform();
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    for part in ["silico fake kernel 1", &platform, step] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    dir.join(&hex[..2]).join(&hex[2..])
}

/// An entry's payload, as the module documents it (postcard is structural).
#[derive(Serialize, Deserialize)]
struct Entry {
    key: [u8; 32],
    brep: Vec<u8>,
    valid: bool,
    history: Option<History>,
}

fn decode(bytes: &[u8]) -> Entry {
    let payload = &bytes[MAGIC.len() + 32..];
    postcard::from_bytes(payload).unwrap()
}

/// An entry's bytes, as the module documents them: the magic, the payload's HMAC-SHA-256
/// under `secret`, and the payload.
fn encode_with(secret: &[u8], entry: &Entry) -> Vec<u8> {
    let payload = postcard::to_allocvec(entry).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
    mac.update(&payload);
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&mac.finalize().into_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// An entry's bytes, sealed with the secret of the cache in `dir`, as its own writer would.
fn encode(dir: &Path, entry: &Entry) -> Vec<u8> {
    encode_with(&std::fs::read(secret_of(dir)).unwrap(), entry)
}

/// The first session's model, built into `dir`.
fn seed(dir: &Path) -> Arc<Model> {
    let model = published(&session(dir), checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    model
}

// ---------------------------------------------------------------------------------------
// Entries that aren't regular files.

// Once: `Cache::get` opens the entry with `std::fs::read`, which on a FIFO blocks in
// open(2) until a writer appears. The recompute thread reads every entry before any step
// runs, so the recompute never finishes, cancel and supersede can't reach it, and every later
// recompute of the document blocks on the same FIFO. A symlink to a FIFO does the same, and
// a symlink to /dev/zero reads until memory runs out (not run here: it would exhaust the
// machine's memory). Expected: a non-regular file is a miss with a warning.
#[test]
fn entry_replaced_by_a_fifo_is_a_miss_not_a_hang() {
    let dir = Scratch::new("fifo");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    assert!(
        path.is_file(),
        "the derived entry path is wrong: {}",
        path.display()
    );
    std::fs::remove_file(&path).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    let second = session(&dir.0);
    let run = second.start(checked(EXAMPLE));
    let outcome = run.wait_timeout(Duration::from_secs(5));
    let hung = outcome.is_err();
    // Free the blocked reader whatever happened, so the test's threads end: open the FIFO
    // for writing (pairing with the reader) and close it, giving it end of file.
    let unblock = {
        let path = path.clone();
        std::thread::spawn(move || {
            let _ = std::fs::OpenOptions::new().write(true).open(&path);
        })
    };
    if let Err(run) = outcome {
        let _ = run.wait_timeout(Duration::from_secs(5));
    } else {
        // The cache didn't open it: open it ourselves so the writer above returns.
        let _ = std::fs::File::open(&path);
    }
    let _ = unblock.join();
    assert!(
        !hung,
        "a FIFO in place of an entry hung the recompute (still waiting after 5 s)"
    );
}

#[test]
fn entry_that_is_a_directory_is_a_miss_and_every_recompute_still_builds() {
    let dir = Scratch::new("entry-dir");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("inside"), b"x").unwrap();
    for _ in 0..2 {
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(calls(&next), ["MakeBox"]);
        assert_eq!(hole(&model), HOLE);
    }
    // The directory is left as it was: never removed, never written into.
    assert!(path.join("inside").is_file());
    // And the partial files whose renames it refused were removed.
    let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains(".partial-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn an_entry_larger_than_any_entry_is_a_miss_without_reading_it() {
    // A sparse file of 64 GiB at the block's entry: read whole, it would exhaust memory.
    let dir = Scratch::new("huge");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    let sound = std::fs::read(&path).unwrap();
    std::fs::File::create(&path)
        .unwrap()
        .set_len(64 << 30)
        .unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox"]);
    assert_eq!(hole(&model), HOLE);
    assert_eq!(std::fs::read(&path).unwrap(), sound);
}

#[test]
fn unreadable_entry_is_a_miss_and_is_written_over() {
    let dir = Scratch::new("unreadable");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    let sound = std::fs::read(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() {
        // Running as root: permissions don't bite.
        return;
    }
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox"]);
    assert_eq!(hole(&model), HOLE);
    // Renamed over: the entry is readable and sound again.
    assert_eq!(std::fs::read(&path).unwrap(), sound);
}

#[test]
fn entry_symlinked_to_another_keys_entry_is_a_miss() {
    let dir = Scratch::new("symlink-other");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let (hole_key, _) = built(&model, "m3tq8v0z");
    let path = entry_path(&dir.0, &box_key);
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(entry_path(&dir.0, &hole_key), &path).unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox"]);
    assert_eq!(hole(&model), HOLE);
    assert_eq!(
        FakeKernel::describe(&built(&model, "k7f2qx3m").1.shape).unwrap(),
        "box(40, 20, 10)"
    );
    // The hole's entry the link pointed to is still sound: the next session hits it.
    let third = session(&dir.0);
    published(&third, checked(EXAMPLE));
    assert!(calls(&third).is_empty(), "{:?}", calls(&third));
}

// ---------------------------------------------------------------------------------------
// Where the cache writes.

// Once: `write_entry` makes its partial file with `File::create` at a predictable name
// (`.partial-<pid>-<n>`, n counting from 0), which follows a symlink already there: the
// target, anywhere the user can write, is truncated and filled with the entry's bytes, and
// the rename then leaves the entry as a symlink to it. A project archive that unpacks
// `.silico-cache/<xx>/.partial-<pid>-<n>` links (for every pid) destroys the file they
// point at the first time the document is rebuilt. Expected: the partial file is created
// exclusively (O_CREAT | O_EXCL, which doesn't follow a link), so the victim is untouched.
#[test]
fn a_planted_partial_file_symlink_doesnt_clobber_a_file_outside_the_cache() {
    // Learn the entries' subdirectories from a probe cache: keys depend only on the kernel,
    // the platform, and the steps.
    let probe = Scratch::new("partial-probe");
    seed(&probe.0);
    let subdirs: Vec<String> = std::fs::read_dir(&probe.0)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!subdirs.is_empty());
    let dir = Scratch::new("partial-symlink");
    let outside = Scratch::new("partial-victim");
    std::fs::create_dir_all(&outside.0).unwrap();
    let victim = outside.0.join("precious.txt");
    std::fs::write(&victim, b"the user's work").unwrap();
    let pid = std::process::id();
    for sub in &subdirs {
        let sub = dir.0.join(sub);
        std::fs::create_dir_all(&sub).unwrap();
        // The write counter is the process's: other tests in the same process may have
        // used some numbers, so cover plenty.
        for n in 0..2000 {
            std::os::unix::fs::symlink(&victim, sub.join(format!(".partial-{pid}-{n}"))).unwrap();
        }
    }
    let engine = session(&dir.0);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"the user's work",
        "a cache write followed a planted symlink and overwrote a file outside the cache"
    );
}

#[test]
fn a_read_only_subdirectory_fails_nothing_and_other_entries_are_kept() {
    let dir = Scratch::new("ro-subdir");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    std::fs::remove_file(&path).unwrap();
    let sub = path.parent().unwrap().to_owned();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(sub.join("probe"), b"").is_ok() {
        return; // root
    }
    for _ in 0..2 {
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(hole(&model), HOLE);
        assert_eq!(calls(&next), ["MakeBox"]);
    }
    // No partial file left behind in the subdirectory it couldn't write.
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let left: Vec<_> = std::fs::read_dir(&sub)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(
        left.iter()
            .all(|p| !p.to_string_lossy().contains(".partial-")),
        "{left:?}"
    );
}

#[test]
fn a_file_where_a_subdirectory_goes_fails_nothing() {
    let dir = Scratch::new("file-subdir");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    let sub = path.parent().unwrap().to_owned();
    std::fs::remove_dir_all(&sub).unwrap();
    std::fs::write(&sub, b"not a directory").unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    // Whatever else shared that subdirectory missed too; nothing failed.
    assert!(calls(&next).contains(&"MakeBox"));
}

#[test]
fn a_cache_directory_removed_while_a_step_runs_is_made_again() {
    let dir = Scratch::new("removed-mid-run");
    let engine = session(&dir.0);
    let hold = engine.kernel().hold(|op| op.name() == "Cut");
    let run = engine.start(checked(EXAMPLE));
    // Wait until the box, the pin, and its move are written; the cut is held.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while files(&dir.0).len() < 3 {
        assert!(std::time::Instant::now() < deadline, "{:?}", files(&dir.0));
        std::thread::yield_now();
    }
    assert!(engine.kernel().wait_running(1, Duration::from_secs(10)));
    std::fs::remove_dir_all(&dir.0).unwrap();
    hold.release();
    let model = match run.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        _ => panic!("not published"),
    };
    assert_eq!(hole(&model), HOLE);
    let (hole_key, _) = built(&model, "m3tq8v0z");
    assert!(entry_path(&dir.0, &hole_key).is_file());
    drop(engine);
    // The others were lost with the directory; the cut is a hit.
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox", "MakeCylinder", "Translate"]);
    assert_eq!(hole(&again), HOLE);
}

#[test]
fn a_cache_directory_that_is_a_dangling_symlink_fails_nothing() {
    let dir = Scratch::new("dangling");
    std::os::unix::fs::symlink(dir.0.with_extension("nowhere"), &dir.0).unwrap();
    let engine = session(&dir.0);
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    let _ = std::fs::remove_file(&dir.0);
}

// ---------------------------------------------------------------------------------------
// Damage at every byte.

#[test]
fn an_entry_cut_at_every_length_or_with_any_byte_flipped_is_a_miss() {
    let dir = Scratch::new("every-byte");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    let sound = std::fs::read(&path).unwrap();
    let mut damaged: Vec<(String, Vec<u8>)> = Vec::new();
    for len in 0..sound.len() {
        damaged.push((format!("cut at {len}"), sound[..len].to_vec()));
    }
    for i in 0..sound.len() {
        let mut bytes = sound.clone();
        bytes[i] ^= 0x80;
        damaged.push((format!("byte {i} flipped"), bytes));
    }
    let mut extra = sound.clone();
    extra.push(0);
    damaged.push(("a byte appended".to_owned(), extra));
    for (what, bytes) in damaged {
        std::fs::write(&path, &bytes).unwrap();
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(calls(&next), ["MakeBox"], "{what}");
        assert_eq!(hole(&model), HOLE, "{what}");
        assert_eq!(std::fs::read(&path).unwrap(), sound, "{what}");
    }
}

// Once: a cache hit's history is never checked. The core checks every history a worker
// sends (`History::validate`, kernel.rs `read_built`: it must be for the operation's
// inputs and name only elements the result has) and calls one that isn't a protocol error;
// a history read back from disk skips that, though it crossed a process boundary just as a
// worker's did. An entry with a sound seal (written by another build of the core, or a
// tool, of the same user's) holding a cut's history for five inputs, naming Face999, is
// published as the cut's history. Expected: an entry whose history doesn't validate for its
// step is a miss, rebuilt.
#[test]
fn an_entry_with_a_sound_seal_but_an_inconsistent_history_is_a_miss() {
    let dir = Scratch::new("forged-history");
    let model = seed(&dir.0);
    let (hole_key, hole_built) = built(&model, "m3tq8v0z");
    let path = entry_path(&dir.0, &hole_key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    let history = entry.history.as_mut().expect("a cut has a history");
    assert_eq!(history.inputs.len(), 2);
    history
        .inputs
        .extend((0..3).map(|_| InputHistory::default()));
    history.ends.start = vec![999];
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    let (_, got) = built(&model, "m3tq8v0z");
    let checked = got.history.as_ref().map(|h| h.validate(2));
    assert_eq!(
        checked,
        Some(Ok(())),
        "a cache hit published a history the core refuses from a worker"
    );
    assert_eq!(got.history, hole_built.history);
    assert_eq!(calls(&next), ["Cut"]);
}

// A step's entry carrying the wrong kind of result: a make step's entry (no history) holding
// a history, or an operation's holding none. Only a forged or buggy writer makes these; a
// hit must still be shaped like what the step makes.
// Once: same root cause as above, nothing checks a hit's shape against its step.
#[test]
fn an_operations_entry_with_no_history_is_a_miss() {
    let dir = Scratch::new("forged-no-history");
    let model = seed(&dir.0);
    let (hole_key, hole_built) = built(&model, "m3tq8v0z");
    let path = entry_path(&dir.0, &hole_key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    entry.history = None;
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    let (_, got) = built(&model, "m3tq8v0z");
    assert_eq!(
        got.history, hole_built.history,
        "a cut published from the cache with no history"
    );
}

/// The example rebuilt by a new session after `edit` was made to `feature`'s entry (sealed
/// as its writer would), and the session's calls.
fn forged(
    name: &str,
    feature: &str,
    edit: impl FnOnce(&mut Entry),
) -> (Vec<&'static str>, Arc<Model>) {
    let dir = Scratch::new(name);
    let model = seed(&dir.0);
    let (key, _) = built(&model, feature);
    let path = entry_path(&dir.0, &key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    edit(&mut entry);
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    (calls(&next), model)
}

#[test]
fn a_hit_keeps_the_validity_the_worker_reported() {
    // The fake makes only valid shapes: an entry saying the block isn't valid is a hit, and
    // says so, rather than vouching for a shape the worker refused to.
    let (calls, model) = forged("invalid", "k7f2qx3m", |entry| entry.valid = false);
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(built(&model, "k7f2qx3m").1.shape.valid(), Some(false));
}

#[test]
fn each_way_a_hits_history_can_misfit_its_step_is_a_miss() {
    // Feature, the step its entry is for, and one fault each.
    type Fault = fn(&mut Entry);
    let faults: [(&str, &str, &str, Fault); 5] = [
        ("cut-extra-input", "m3tq8v0z", "Cut", |e| {
            e.history
                .as_mut()
                .unwrap()
                .inputs
                .push(InputHistory::default());
        }),
        ("cut-unknown-end", "m3tq8v0z", "Cut", |e| {
            e.history.as_mut().unwrap().ends.start = vec![999];
        }),
        ("move-extra-input", "a91cw0de", "Translate", |e| {
            e.history
                .as_mut()
                .unwrap()
                .inputs
                .push(InputHistory::default());
        }),
        ("move-no-history", "a91cw0de", "Translate", |e| {
            e.history = None
        }),
        ("make-empty-history", "k7f2qx3m", "MakeBox", |e| {
            e.history = Some(History::default());
        }),
    ];
    for (name, feature, step, fault) in faults {
        let fresh = seed(&Scratch::new(&format!("{name}-fresh")).0);
        let (calls, model) = forged(name, feature, fault);
        assert_eq!(calls, [step], "{name}");
        assert_eq!(
            built(&model, feature).1.history,
            built(&fresh, feature).1.history,
            "{name}"
        );
    }
}

#[test]
fn a_makes_entry_with_a_history_is_a_miss() {
    let dir = Scratch::new("forged-make-history");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let (_, hole_built) = built(&model, "m3tq8v0z");
    let path = entry_path(&dir.0, &box_key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    assert!(entry.history.is_none());
    entry.history = hole_built.history.clone();
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox"]);
    assert_eq!(built(&model, "k7f2qx3m").1.history, None);
}

// Once: an entry's checksum was a bare SHA-256, and its key a hash of public parts (the
// kernel's identity, the same for every user of a release, the platform, and the document's
// steps), so anyone could write an entry every check passes. A `.silico-cache/` committed
// or copied with a document, holding another shape under a step's key, was published as
// that feature's result, and vouched valid, with no kernel call; nothing in the document's
// text shows it. Expected: an entry sealed with any secret but this user's is a miss.
#[test]
fn a_planted_entry_with_another_shape_isnt_taken_for_the_features_result() {
    let dir = Scratch::new("planted-shape");
    let model = seed(&dir.0);
    let (hole_key, _) = built(&model, "m3tq8v0z");
    let (_, block) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &hole_key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    entry.brep = block.shape.brep().as_bytes().to_vec();
    // Sealed by someone else: the old format's bare checksum, or another secret.
    let unsealed = {
        let payload = postcard::to_allocvec(&entry).unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&Sha256::digest(&payload));
        bytes.extend_from_slice(&payload);
        bytes
    };
    for (who, planted) in [
        ("a bare checksum", unsealed),
        ("another secret", encode_with(&[7; 32], &entry)),
    ] {
        std::fs::write(&path, planted).unwrap();
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(
            hole(&model),
            HOLE,
            "{who}: a planted entry replaced the cut"
        );
        assert_eq!(calls(&next), ["Cut"], "{who}");
    }
    // The same entry sealed with the user's own secret is taken (so the planted ones
    // above were refused for their seal alone).
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    assert_eq!(hole(&published(&next, checked(EXAMPLE))), "box(40, 20, 10)");
    assert!(calls(&next).is_empty());
}

// ---------------------------------------------------------------------------------------
// Keys.

/// The fake kernel under an identity of the test's choosing.
struct Named(FakeKernel, String);

impl Kernel for Named {
    fn identity(&self) -> &str {
        &self.1
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

// Once: an entry's key hashes the kernel's identity, the platform, and the step joined by
// newlines, with no lengths, so a newline in either moves the boundary: kernel `k\np` on
// platform `q` and kernel `k` on platform `p\nq` hash the same bytes and share every entry,
// though the module promises a result is used "only by the same kernel ... on the same kind
// of machine". `Kernel::identity` is any implementer's string and `Cache::for_platform` takes
// any string. Expected: the second session misses every step.
#[test]
fn identity_and_platform_with_newlines_dont_share_entries() {
    let dir = Scratch::new("newline-key");
    let first = Recompute::with_cache(
        Named(FakeKernel::new(2).unwrap(), "k\np".to_owned()),
        Cache::for_platform(&dir.0, "q", &dir.secret()),
    );
    published(&first, checked(EXAMPLE));
    let second = Recompute::with_cache(
        Named(FakeKernel::new(2).unwrap(), "k".to_owned()),
        Cache::for_platform(&dir.0, "p\nq", &dir.secret()),
    );
    published(&second, checked(EXAMPLE));
    assert_eq!(
        second.kernel().0.calls().len(),
        4,
        "another kernel on another platform was served the first's entries"
    );
}

#[test]
fn an_empty_identity_and_platform_still_key_by_step() {
    let dir = Scratch::new("empty-key");
    let engine = Recompute::with_cache(
        Named(FakeKernel::new(2).unwrap(), String::new()),
        Cache::for_platform(&dir.0, "", &dir.secret()),
    );
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    let model = published(&engine, example_with_pin("3"));
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
}

// ---------------------------------------------------------------------------------------
// Sessions racing.

#[test]
fn sessions_racing_on_one_cache_leave_only_sound_entries() {
    let dir = Scratch::new("race");
    let radii = ["2.5", "3", "1", "2.5", "3", "1", "2.5", "3"];
    for round in 0..10 {
        let barrier = Arc::new(std::sync::Barrier::new(radii.len()));
        let threads: Vec<_> = radii
            .iter()
            .map(|radius| {
                let dir = dir.0.clone();
                let barrier = Arc::clone(&barrier);
                let radius = (*radius).to_owned();
                std::thread::spawn(move || {
                    let engine = session(&dir);
                    barrier.wait();
                    let model = published(&engine, example_with_pin(&radius));
                    hole(&model)
                })
            })
            .collect();
        for (thread, radius) in threads.into_iter().zip(radii) {
            assert_eq!(
                thread.join().unwrap(),
                format!("cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))"),
                "round {round}"
            );
        }
    }
    // One box, three pins, three moves, three cuts: ten entries, no partial files left.
    let all = files(&dir.0);
    assert!(
        all.iter()
            .all(|p| !p.to_string_lossy().contains(".partial-")),
        "{all:?}"
    );
    assert_eq!(all.len(), 10, "{all:?}");
    // And all of them are hits, with the right shapes.
    let last = session(&dir.0);
    for radius in ["1", "2.5", "3"] {
        let model = published(&last, example_with_pin(radius));
        assert_eq!(
            hole(&model),
            format!("cut(box(40, 20, 10), translate(cylinder({radius}, 12), 20, 10, -1))")
        );
    }
    assert!(calls(&last).is_empty(), "{:?}", calls(&last));
}

#[test]
fn a_reader_racing_a_writer_never_takes_a_partial_entry() {
    // One thread rewrites an entry with alternating sound bytes and garbage, as fast as it
    // can, by rename (as the cache does); sessions reading it only ever get the right
    // shape.
    let dir = Scratch::new("race-reader");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    let path = entry_path(&dir.0, &box_key);
    let sound = std::fs::read(&path).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (path, stop, sound) = (path.clone(), Arc::clone(&stop), sound.clone());
        std::thread::spawn(move || {
            let tmp = path.with_extension("tmp");
            let mut n = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let bytes = if n.is_multiple_of(2) {
                    sound.clone()
                } else {
                    let mut b = sound.clone();
                    b[sound.len() - 1] ^= 1;
                    b
                };
                std::fs::write(&tmp, bytes).unwrap();
                let _ = std::fs::rename(&tmp, &path);
                n += 1;
            }
        })
    };
    for _ in 0..50 {
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert_eq!(hole(&model), HOLE);
        assert_eq!(
            FakeKernel::describe(&built(&model, "k7f2qx3m").1.shape).unwrap(),
            "box(40, 20, 10)"
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().unwrap();
}

// ---------------------------------------------------------------------------------------
// Hits and failures together.

#[test]
fn a_hit_downstream_of_a_step_that_now_fails_is_blocked_as_solidworks_does() {
    let dir = Scratch::new("hit-after-failure");
    let model = seed(&dir.0);
    let (box_key, _) = built(&model, "k7f2qx3m");
    std::fs::remove_file(entry_path(&dir.0, &box_key)).unwrap();
    let next = session(&dir.0);
    next.kernel().refuse(|op| {
        (op.name() == "MakeBox").then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
    });
    let model = published(&next, checked(EXAMPLE));
    // The cut's entry is still on disk, but the block failed: the hole is blocked on it, and
    // the pin stays a body.
    assert_eq!(calls(&next), ["MakeBox"]);
    assert!(matches!(
        model.outcome(&FeatureId::new("k7f2qx3m").unwrap()),
        Some(FeatureOutcome::Failed(_))
    ));
    assert_eq!(
        model.outcome(&FeatureId::new("m3tq8v0z").unwrap()),
        Some(&FeatureOutcome::Blocked {
            on: FeatureId::new("k7f2qx3m").unwrap()
        })
    );
    assert_eq!(model.bodies(), [FeatureId::new("a91cw0de").unwrap()]);
    // And no failure was written: the next session builds the block.
    let third = session(&dir.0);
    let model = published(&third, checked(EXAMPLE));
    assert_eq!(calls(&third), ["MakeBox"]);
    assert_eq!(hole(&model), HOLE);
}

#[test]
fn an_entry_written_by_a_superseded_recompute_is_sound() {
    let dir = Scratch::new("superseded");
    let engine = session(&dir.0);
    let hold = engine.kernel().hold(|op| op.name() == "Cut");
    let first = engine.start(checked(EXAMPLE));
    assert!(engine.kernel().wait_running(1, Duration::from_secs(10)));
    let second = engine.start(example_with_pin("3"));
    hold.release();
    assert!(matches!(
        first.wait_timeout(Duration::from_secs(10)),
        Ok(Ok(RunOutcome::Superseded))
    ));
    let model = match second.wait_timeout(Duration::from_secs(10)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        _ => panic!("not published"),
    };
    assert_eq!(
        hole(&model),
        "cut(box(40, 20, 10), translate(cylinder(3, 12), 20, 10, -1))"
    );
    drop(engine);
    let next = session(&dir.0);
    let model = published(&next, checked(EXAMPLE));
    assert_eq!(hole(&model), HOLE);
    // Whatever the superseded run wrote is sound; whatever it didn't is built now.
    let again = session(&dir.0);
    published(&again, checked(EXAMPLE));
    assert!(calls(&again).is_empty());
}
