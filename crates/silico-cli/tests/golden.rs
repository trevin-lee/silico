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
fn the_document_saved_under_naming_scheme_5_rebuilds_with_every_name_finding_its_edge() {
    // silico-core's tests/contract/naming-scheme-5.silico (M4.0, protected): saved by `silico
    // fmt` under scheme 5. On the real kernel every name finds its edge (a name that finds
    // none is a `reference_broken` problem) and every feature rebuilds to its fingerprint,
    // whose volumes are worked by hand: the block 40 x 20 x 10 = 8000; the pin
    // pi 2.5^2 12 = 235.619449; the cut 8000 - pi 2.5^2 10 = 7803.650459; the tab
    // 10 x 10 x 5 = 500. A later scheme's migration is held to this file.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/naming-scheme-5.silico");
    let committed = std::fs::read_to_string(&fixture).unwrap();
    for (volume, at) in [
        ("8000", "\"volume\": 8000,"),
        ("235.619449", "\"volume\": 235.619449,"),
        ("7803.650459", "\"volume\": 7803.650459,"),
        ("500", "\"volume\": 500,"),
    ] {
        assert!(committed.contains(at), "no volume {volume}");
    }
    let scratch = Scratch::new("naming-scheme-5");
    let file = scratch.0.join("naming-scheme-5.silico");
    std::fs::write(&file, &committed).unwrap();
    let checked = silico(&scratch, &["check", "--json"], &file);
    let out = String::from_utf8_lossy(&checked.stdout);
    assert_eq!(
        checked.status.code(),
        Some(0),
        "{out}{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(
        out.contains(r#""ok":true"#) && out.contains(r#""total":0"#),
        "{out}"
    );
    let canonical = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(
        canonical.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&canonical.stderr)
    );

    // And the check can fail: a rim named from the block's side at x = 0, which the pin
    // never meets, finds no edge.
    let wrong = committed.replacen(
        "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)",
        "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face1, G a91cw0de.cylinder.Face1)",
        1,
    );
    assert_ne!(wrong, committed);
    std::fs::write(&file, &wrong).unwrap();
    let broken = silico(&scratch, &["check", "--json"], &file);
    let out = String::from_utf8_lossy(&broken.stdout);
    assert_eq!(broken.status.code(), Some(1), "{out}");
    assert!(
        out.contains(r#""code":"reference_broken""#) && out.contains(r#""feature":"f0000001""#),
        "{out}"
    );
}

#[test]
fn a_golden_model_saved_before_naming_was_recorded_is_checked_clean_and_formatted_to_its_bytes() {
    // M4.0: a document saved before the header recorded its naming scheme reads as scheme
    // 5. `check` finds nothing wrong with it (as with any layout that isn't canonical);
    // `fmt --check` says `fmt` would rewrite it; `fmt` writes exactly the committed model,
    // its one new line `"naming": 5`, and nothing else; and then leaves it as it is.
    for model in ["chamfered-box.silico", "plate-with-boss.silico"] {
        let scratch = Scratch::new(&format!(
            "before-naming-{}",
            model.trim_end_matches(".silico")
        ));
        let committed = std::fs::read_to_string(golden().join(model)).unwrap();
        let line = "  \"naming\": 5,\n";
        assert_eq!(committed.matches(line).count(), 1, "{model}");
        let before = committed.replacen(line, "", 1);
        let file = scratch.0.join(model);
        std::fs::write(&file, &before).unwrap();
        let checked = silico(&scratch, &["check"], &file);
        assert_eq!(
            checked.status.code(),
            Some(0),
            "{model}: {}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert!(checked.stderr.is_empty(), "{model}");
        let dry = silico(&scratch, &["fmt", "--check"], &file);
        assert_eq!(dry.status.code(), Some(1), "{model}");
        assert_eq!(
            String::from_utf8_lossy(&dry.stderr),
            format!(
                "{}: not in canonical form; `silico fmt` would rewrite it\n",
                file.display()
            ),
            "{model}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before, "{model}");
        let saved = silico(&scratch, &["fmt"], &file);
        assert_eq!(
            saved.status.code(),
            Some(0),
            "{model}: {}",
            String::from_utf8_lossy(&saved.stderr)
        );
        // No fingerprint or solve written: the field is all that changed.
        assert!(
            saved.stderr.is_empty(),
            "{model}: {}",
            String::from_utf8_lossy(&saved.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            committed,
            "{model}"
        );
        let again = silico(&scratch, &["fmt", "--check"], &file);
        assert_eq!(again.status.code(), Some(0), "{model}");
        let again = silico(&scratch, &["fmt"], &file);
        assert_eq!(again.status.code(), Some(0), "{model}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            committed,
            "{model}"
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

#[test]
fn the_golden_models_centres_are_where_their_material_is_by_hand() {
    // Each fingerprint's centre of mass, worked by hand and held to the nanometre the file
    // writes: the plate 60 × 30 × 5 at its middle, (30, 15, 2.5); its first hole, r 1.5
    // through its 5 at (15, 15), takes V_h = 11.25π from it, so x = (9000 · 30 − 15 V_h) /
    // (9000 − V_h) = 30.0591371 (y and z unchanged, the hole being centred on both); the
    // fused boxes, a 10 mm cube at (5, 5, 5) and a 10 × 10 × 5 slab beside it at
    // (15, 5, 2.5), (1000 · 5 + 500 · 15) / 1500 = 8.333333 and (5000 + 1250) / 1500 =
    // 4.166667; and the plate with a boss's pad, 60 × 30 × 8 about the origin, (0, 0, 4).
    use std::f64::consts::PI;
    let centre = |model: &str, feature: &str| {
        let text = std::fs::read_to_string(golden().join(model)).unwrap();
        let document = silico_core::document::read(text.as_bytes()).unwrap();
        document
            .features
            .iter()
            .find(|f| f.id.as_str() == feature)
            .and_then(|f| f.fingerprint.as_ref())
            .and_then(|f| f.center)
            .map(|c| c.map(silico_core::document::Length::nm))
            .unwrap_or_else(|| panic!("{model}: {feature} has no centre"))
    };
    let nm = |mm: f64| (mm * 1e6).round() as i64;
    let hole = 11.25 * PI;
    for (model, feature, want) in [
        ("plate-with-holes.silico", "pppppppp", [30.0, 15.0, 2.5]),
        (
            "plate-with-holes.silico",
            "c0000001",
            [(9000.0 * 30.0 - 15.0 * hole) / (9000.0 - hole), 15.0, 2.5],
        ),
        (
            "fused-boxes.silico",
            "rrrrrrrr",
            [12.5 / 1.5, 5.0, 6.25 / 1.5],
        ),
        ("plate-with-boss.silico", "p0000001", [0.0, 0.0, 4.0]),
    ] {
        assert_eq!(centre(model, feature), want.map(nm), "{model}: {feature}");
    }
}

/// `text` with every fingerprint's centre taken out (`"center": [x, y, z], `), as Silico
/// wrote fingerprints before #389; a circle's centre, a point's ID, is left.
fn without_centres(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(r#""center": ["#) {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let end = after.find("], ").unwrap() + "], ".len();
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The `warning: CODE:` lines `stderr` says, by code.
fn warned(stderr: &str, code: &str) -> usize {
    stderr
        .lines()
        .filter(|line| line.contains(&format!(": warning: {code}: ")))
        .count()
}

#[test]
fn a_golden_model_saved_before_fingerprints_held_centres_checks_clean_and_formats_to_its_bytes() {
    // Fable's decision of 2026-11-09: a fingerprint stored before #389 holds its box and no
    // centre. `check` compares nothing on a centre it never recorded, so every feature
    // checks clean (exit 0, no drift), and warns `fingerprint_incomplete` once for each
    // fingerprint the model holds; `fmt` writes exactly the committed model, its centres
    // back in; and then nothing warns.
    for model in MODELS {
        let scratch = Scratch::new(&format!(
            "before-centres-{}",
            model.trim_end_matches(".silico")
        ));
        let committed = std::fs::read_to_string(golden().join(model)).unwrap();
        let centres = committed.matches(r#""center": ["#).count();
        assert!(centres > 0, "{model}");
        let before = without_centres(&committed);
        assert!(!before.contains(r#""center": ["#), "{model}");
        let file = scratch.0.join(model);
        std::fs::write(&file, &before).unwrap();
        let checked = silico(&scratch, &["check"], &file);
        let said = String::from_utf8_lossy(&checked.stderr).into_owned();
        assert_eq!(checked.status.code(), Some(0), "{model}: {said}");
        assert_eq!(
            warned(&said, "fingerprint_incomplete"),
            centres,
            "{model}: {said}"
        );
        assert_eq!(
            said.lines().count(),
            centres,
            "{model}: nothing else said: {said}"
        );
        let saved = silico(&scratch, &["fmt"], &file);
        assert_eq!(
            saved.status.code(),
            Some(0),
            "{model}: {}",
            String::from_utf8_lossy(&saved.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            committed,
            "{model}"
        );
        let again = silico(&scratch, &["check"], &file);
        assert_eq!(again.status.code(), Some(0), "{model}");
        assert!(
            again.stderr.is_empty(),
            "{model}: {}",
            String::from_utf8_lossy(&again.stderr)
        );
    }
}

#[test]
fn m4s_live_bracket_saved_before_centres_checks_with_no_error_and_a_warning_a_feature() {
    // Main's (2026-11-09): M4's live bracket, as recorded (docs/walkthrough/m4-live,
    // saved 2026-10-29, before #389), opened all-red before Fable's decision, each feature
    // drifting on `center none → [x, y, z]`. Now: no error, and one
    // `fingerprint_incomplete` for each of its six fingerprints; formatted (in a copy, the
    // record kept as it is), each centre written and nothing warns.
    let recorded =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/walkthrough/m4-live/bracket.silico");
    let text = std::fs::read_to_string(&recorded).unwrap();
    let fingerprints = text.matches(r#""fingerprint": {"#).count();
    assert_eq!(fingerprints, 6);
    assert!(
        !text.contains(r#""center": ["#),
        "the recorded bracket holds no centre"
    );
    let scratch = Scratch::new("m4-live-bracket");
    let file = scratch.0.join("bracket.silico");
    std::fs::write(&file, &text).unwrap();
    let checked = silico(&scratch, &["check"], &file);
    let said = String::from_utf8_lossy(&checked.stderr).into_owned();
    assert_eq!(checked.status.code(), Some(0), "{said}");
    // Each line said a warning of the new code: no error, no drift.
    assert!(
        said.lines()
            .all(|line| line.contains(": warning: fingerprint_incomplete: ")),
        "{said}"
    );
    assert_eq!(
        warned(&said, "fingerprint_incomplete"),
        fingerprints,
        "{said}"
    );
    assert_eq!(
        said.lines().count(),
        fingerprints,
        "nothing else said: {said}"
    );
    let saved = silico(&scratch, &["fmt"], &file);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    let formatted = std::fs::read_to_string(&file).unwrap();
    assert_eq!(formatted.matches(r#""center": ["#).count(), fingerprints);
    let again = silico(&scratch, &["check"], &file);
    assert_eq!(again.status.code(), Some(0));
    assert!(
        again.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}
