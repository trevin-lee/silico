//! M2's first exit test: the golden models (`tests/golden/`, protected) save and rebuild
//! byte-identically on every platform. Each is a document in canonical form with its
//! fingerprints, recorded on the Mac. On whatever machine this runs (the Mac, and CI's
//! Linux runner), `silico check` finds nothing wrong with it, so every feature rebuilds to
//! the measures stored with it; `silico fmt --check` finds it canonical; and `silico fmt`
//! leaves its bytes as they are. The fingerprints' volumes were checked by hand when the
//! models were recorded: a chamfer of three edges, a fillet of an edge between a chamfer and
//! a side, an inside corner's fillet, holes' rims, a pointed cone's bore, and M3.6's plate
//! padded from a sketch, two holes pocketed through it, a boss revolved onto it, and a
//! fillet of the pad's front top edge by name (each feature's volume held to its hand-worked
//! value below). The plate's holes are each their own size, so a fillet of the wrong rim
//! measures otherwise; the chamfered box's vertical edges are alike, so which of them its
//! fillet takes is held by silico-kernel's tests/references.rs, not here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
#[path = "common/run.rs"]
mod run;

use std::process::{Command, Output, Stdio};

/// The golden models, by file name.
const MODELS: [&str; 6] = [
    "chamfered-box.silico",
    "example.silico",
    "fused-boxes.silico",
    "plate-with-boss.silico",
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

/// `silico ARGS FILE`, with the kernel and sketch workers beside it and the cache in
/// `scratch`.
fn silico(scratch: &Scratch, args: &[&str], file: &Path) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let worker = exe.with_file_name("silico-kernel-worker");
    let sketches = exe.with_file_name("silico-sketch-worker");
    for worker in [&worker, &sketches] {
        assert!(
            worker.exists(),
            "{} isn't built: build the workspace (pixi run test does)",
            worker.display()
        );
    }
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .arg(file)
        .env("SILICO_KERNEL_WORKER", &worker)
        .env("SILICO_SKETCH_WORKER", &sketches)
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

#[test]
fn the_plate_with_a_boss_measures_what_its_features_make_by_hand() {
    // M3.6's golden model (docs/design/m3.6-sketch-features.md, S10): the 60 × 30 rectangle
    // about the origin padded 8 (14400); holes of radii 3 and 4 pocketed through it, up from
    // its bottom (π (9 + 16) 8 = 200π less); the 5 × 6 rectangle on XZ from (0, 8) turned
    // whole about the sketch's y axis, a boss of radius 5 and height 6 on its top (150π
    // more); and a fillet of radius 1 of the top edge over the plate's front side, 60 long
    // between two square ends, by its name `p0000001.pad.Edge(G
    // s0000001.sketch.Edge.na000001)` ((1 − π/4) × 60 less). Each fingerprint's volume
    // (written to 6 decimals) is the formula's.
    use std::f64::consts::PI;
    let text = std::fs::read_to_string(golden().join("plate-with-boss.silico")).unwrap();
    let document = silico_core::document::read(text.as_bytes()).unwrap();
    let volume = |feature: &str| {
        document
            .features
            .iter()
            .find(|f| f.id.as_str() == feature)
            .and_then(|f| f.fingerprint.as_ref())
            .map(|f| f.volume.to_string().parse::<f64>().unwrap())
            .unwrap()
    };
    for (feature, want) in [
        ("p0000001", 14400.0),
        ("q0000001", 14400.0 - 200.0 * PI),
        ("r0000001", 14400.0 - 50.0 * PI),
        ("f0000001", 14400.0 - 50.0 * PI - (1.0 - PI / 4.0) * 60.0),
    ] {
        assert!(
            (volume(feature) - want).abs() <= 5e-7,
            "{feature}: {} against {want}",
            volume(feature)
        );
    }
    assert!(text.contains("\"p0000001.pad.Edge(G s0000001.sketch.Edge.na000001)\""));
}
