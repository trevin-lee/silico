//! M2's first exit test: the golden models (`tests/golden/`, protected) save and rebuild
//! byte-identically on every platform. Each is a document in canonical form with its
//! fingerprints, recorded on the Mac. On whatever machine this runs (the Mac, and CI's
//! Linux runner), `silico check` finds nothing wrong with it, so every feature rebuilds to
//! the measures stored with it; `silico fmt --check` finds it canonical; and `silico fmt`
//! leaves its bytes as they are. The fingerprints' volumes were checked by hand when the
//! models were recorded: a chamfer of three edges, a fillet of an edge between a chamfer and
//! a side, an inside corner's fillet, holes' rims, a pointed cone's bore.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The golden models, by file name.
const MODELS: [&str; 5] = [
    "chamfered-box.silico",
    "example.silico",
    "fused-boxes.silico",
    "plate-with-holes.silico",
    "pointed-cone.silico",
];

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// A directory of its own for one model, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("golden-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `silico ARGS FILE`, with the kernel worker beside it and the cache in `scratch`.
fn silico(scratch: &Scratch, args: &[&str], file: &Path) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let worker = exe.with_file_name("silico-kernel-worker");
    assert!(
        worker.exists(),
        "{} isn't built: build the workspace (pixi run test does)",
        worker.display()
    );
    Command::new(exe)
        .args(args)
        .arg(file)
        .env("SILICO_KERNEL_WORKER", &worker)
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn every_golden_model_is_listed_here() {
    // A model added to tests/golden/ is checked only once it's listed.
    let mut found: Vec<String> = std::fs::read_dir(golden())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    assert_eq!(found, MODELS);
}

#[test]
fn every_golden_model_rebuilds_to_its_fingerprints_and_formats_to_its_own_bytes() {
    for model in MODELS {
        let scratch = Scratch::new(model.trim_end_matches(".silico"));
        let committed = std::fs::read(golden().join(model)).unwrap();
        let file = scratch.0.join(model);
        std::fs::write(&file, &committed).unwrap();
        // Every feature builds, to the measures stored with it: no failure, no drift.
        let checked = silico(&scratch, &["check"], &file);
        assert_eq!(
            checked.status.code(),
            Some(0),
            "{model}: {}{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        // Canonical, fingerprints and all.
        let canonical = silico(&scratch, &["fmt", "--check"], &file);
        assert_eq!(
            canonical.status.code(),
            Some(0),
            "{model}: {}",
            String::from_utf8_lossy(&canonical.stderr)
        );
        // And a save writes it back byte for byte.
        let saved = silico(&scratch, &["fmt"], &file);
        assert_eq!(
            saved.status.code(),
            Some(0),
            "{model}: {}",
            String::from_utf8_lossy(&saved.stderr)
        );
        assert!(
            std::fs::read(&file).unwrap() == committed,
            "{model}: a save changed its bytes"
        );
    }
}
