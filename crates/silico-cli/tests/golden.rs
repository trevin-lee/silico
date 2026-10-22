//! M2's first exit test: the golden models (`tests/golden/`, protected) save and rebuild
//! byte-identically on every platform. Each is a document in canonical form with its
//! fingerprints, recorded on the Mac. On whatever machine this runs (the Mac, and CI's
//! Linux runner), `silico check` finds nothing wrong with it, so every feature rebuilds to
//! the measures stored with it; `silico fmt --check` finds it canonical; and `silico fmt`
//! leaves its bytes as they are. The fingerprints' volumes were checked by hand when the
//! models were recorded: a chamfer of three edges, a fillet of an edge between a chamfer and
//! a side, an inside corner's fillet, holes' rims, a pointed cone's bore. The plate's holes
//! are each their own size, so a fillet of the wrong rim measures otherwise; the chamfered
//! box's vertical edges are alike, so which of them its fillet takes is held by
//! silico-kernel's tests/references.rs, not here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
#[path = "common/run.rs"]
mod run;

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
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .arg(file)
        .env("SILICO_KERNEL_WORKER", &worker)
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    run::output(cmd)
}

#[test]
fn every_golden_model_is_listed_here() {
    // A model added to tests/golden/ is checked only once it's listed.
    // (Not a dotfile: a `.silico-cache/` a run of silico on a model in place leaves, or
    // Finder's `.DS_Store`, both ignored by git.)
    let mut found: Vec<String> = std::fs::read_dir(golden())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
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
        // And a save leaves it as it is: `fmt` exits clean and writes nothing over it.
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
