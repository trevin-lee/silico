//! `silico export` (docs/cli.md), the CLI's thin client of the `export` command, on the real
//! kernel: a document's bodies written as a 3MF file a slicer reads, read back here from its
//! bytes (its entries are stored, so its model's XML is in them as written). Expected values
//! are by hand: a box's mesh is exactly its 8 corners and 12 triangles, enclosing its
//! volume, 40 × 20 × 10 = 8000 mm³. Exit codes from docs/cli.md: 0 written, 1 refused for the
//! request or the document, 2 the command couldn't run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;
#[path = "common/threemf.rs"]
mod threemf;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use threemf::{model, printable};

const BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(std::fs::canonicalize(&dir).unwrap())
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "cache-home" && n != ".silico-cache")
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `silico export ARGS`, run in `dir` with the workspace's workers.
fn silico_export(dir: &Path, args: &[&str]) -> Output {
    silico_export_with(dir, args, None)
}

/// [`silico_export`], but with `SILICO_SKETCH_WORKER` naming `sketch_worker` if given.
fn silico_export_with(dir: &Path, args: &[&str], sketch_worker: Option<&Path>) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.arg("export")
        .args(args)
        .current_dir(dir)
        .env(
            "SILICO_KERNEL_WORKER",
            exe.with_file_name("silico-kernel-worker"),
        )
        .env(
            "SILICO_STEP_WORKER",
            exe.with_file_name("silico-step-worker"),
        )
        .env(
            "SILICO_SKETCH_WORKER",
            sketch_worker.map_or_else(
                || exe.with_file_name("silico-sketch-worker"),
                Path::to_owned,
            ),
        )
        .env("XDG_CACHE_HOME", dir.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    run::output(cmd)
}

#[test]
fn a_box_is_exported_as_its_8_corners_and_12_triangles_enclosing_its_volume() {
    let dir = Scratch::new("box");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    let out = silico_export(&dir.0, &["part.silico", "part.3mf"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let bytes = std::fs::read(dir.0.join("part.3mf")).unwrap();
    assert_eq!(
        stdout,
        format!(
            "wrote part.3mf ({} bytes)\nk7f2qx3m \"Block\": 12 triangles, 8 vertices, volume 8000 mm³\n",
            bytes.len()
        )
    );
    let objects = model(&bytes);
    assert_eq!(objects.len(), 1);
    let object = &objects[0];
    assert_eq!(object.name, "Block");
    assert_eq!((object.vertices.len(), object.triangles.len()), (8, 12));
    let mut corners = object.vertices.clone();
    corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut want: Vec<[f64; 3]> = (0..8)
        .map(|i| {
            [
                if i & 4 != 0 { 40.0 } else { 0.0 },
                if i & 2 != 0 { 20.0 } else { 0.0 },
                if i & 1 != 0 { 10.0 } else { 0.0 },
            ]
        })
        .collect();
    want.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(corners, want);
    assert_eq!(printable(object), 8000.0);
    // The same bytes every run: an archive whose bytes depend on the mesh and names alone.
    let again = silico_export(&dir.0, &["--replace", "part.silico", "part.3mf"]);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(std::fs::read(dir.0.join("part.3mf")).unwrap(), bytes);
}

#[test]
fn a_file_already_there_is_kept_without_replace_and_the_refusal_says_why() {
    let dir = Scratch::new("exists");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    std::fs::write(dir.0.join("part.3mf"), b"someone's").unwrap();
    let out = silico_export(&dir.0, &["part.silico", "part.3mf"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(
        stderr,
        "part.silico: invalid_input: \"part.3mf\" exists (parameter path). Suggestion: give \
         --replace to replace it, or another path.\n"
    );
    assert_eq!(std::fs::read(dir.0.join("part.3mf")).unwrap(), b"someone's");
    assert!(out.stdout.is_empty());
}

#[test]
fn every_reason_an_export_is_refused_is_said_and_nothing_written() {
    // A tolerance out of range, a path in no directory there is, and a feature the
    // document hasn't: each said, in the design's order, nothing written (exit 1). (A path
    // of neither format is the CLI's usage error, before any of these.)
    let dir = Scratch::new("refused");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    let out = silico_export(
        &dir.0,
        &[
            "--tolerance=0",
            "--features=zzzzzzzz",
            "part.silico",
            "nowhere/part.3mf",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    let parameters: Vec<&str> = stderr
        .lines()
        .map(|l| {
            l.split("(parameter ")
                .nth(1)
                .unwrap()
                .split(')')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(parameters, ["tolerance", "path", "features.0"], "{stderr}");
    assert_eq!(dir.names(), ["part.silico"]);
}

#[test]
fn a_tolerance_or_feature_that_isnt_one_is_a_usage_error() {
    let dir = Scratch::new("usage");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    for args in [
        &["--tolerance=fine", "part.silico", "part.3mf"][..],
        &["--features=Not-An-ID", "part.silico", "part.3mf"][..],
        &["part.silico"][..],
        &["part.silico", "a.3mf", "b.3mf"][..],
        &["--format=stl", "part.silico", "part.3mf"][..],
    ] {
        let out = silico_export(&dir.0, args);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("usage: silico"), "{args:?}: {stderr}");
    }
    assert_eq!(dir.names(), ["part.silico"]);
}

#[test]
fn the_features_listed_are_written_in_their_order() {
    // Two boxes, the second moved: listed second then first, the objects in that order.
    let two = BOX.replace(
        "\"height\": 10}\n",
        "\"height\": 10},\n    {\"id\": \"b2b2b2b2\", \"type\": \"box\", \"v\": 1, \"name\": \"Other\", \"position\": [100, 0, 0], \"length\": 1, \"width\": 2, \"height\": 3}\n",
    );
    let dir = Scratch::new("listed");
    std::fs::write(dir.0.join("part.silico"), &two).unwrap();
    let out = silico_export(
        &dir.0,
        &["--features=b2b2b2b2,k7f2qx3m", "part.silico", "part.3mf"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let named: Vec<&str> = stdout.lines().skip(1).map(|l| &l[..8]).collect();
    assert_eq!(named, ["b2b2b2b2", "k7f2qx3m"]);
    let objects = model(&std::fs::read(dir.0.join("part.3mf")).unwrap());
    let volumes: Vec<f64> = objects.iter().map(printable).collect();
    assert_eq!(volumes, [6.0, 8000.0]);
}

/// A sketch, then the box: with no sketch worker to be had, the sketch fails with a kernel
/// fault (which says nothing of the document), and the box builds.
const SKETCH_AND_BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Line", "plane": "xy",
     "points": [
       {"id": "pa000000", "x": 0, "y": 0, "construction": false},
       {"id": "pb000000", "x": 10, "y": 0, "construction": false}
     ],
     "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}],
     "circles": [], "arcs": [],
     "constraints": []},
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// What the CLI says of the sketch worker at `/nowhere`: the core's words, then to run
/// `silico export` again.
const NO_SKETCH_WORKER: &str = "worker_unavailable: SILICO_SKETCH_WORKER names \"/nowhere\", \
     which isn't a file. Suggestion: point SILICO_SKETCH_WORKER at the silico-sketch-worker \
     binary, or unset it to use the one beside Silico; run `silico export` again to retry it.";

#[test]
fn a_kernel_fault_beside_an_export_is_said_once_after_it_and_fails_the_command() {
    // The box, asked for, built, so it's written; but the rebuild met a fault, so the
    // command couldn't wholly run (exit 2), and says the fault last, in the CLI's words.
    let dir = Scratch::new("fault-beside");
    std::fs::write(dir.0.join("part.silico"), SKETCH_AND_BOX).unwrap();
    let out = silico_export_with(
        &dir.0,
        &["--features=k7f2qx3m", "part.silico", "part.3mf"],
        Some(Path::new("/nowhere")),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    let bytes = std::fs::read(dir.0.join("part.3mf")).unwrap();
    assert_eq!(
        stdout,
        format!(
            "wrote part.3mf ({} bytes)\nk7f2qx3m \"Block\": 12 triangles, 8 vertices, volume 8000 mm³\n",
            bytes.len()
        )
    );
    assert_eq!(stderr, format!("part.silico: {NO_SKETCH_WORKER}\n"));
}

#[test]
fn every_body_of_a_model_a_fault_stopped_is_refused_saying_the_fault_once() {
    // Every body asked for, but the sketch didn't build, stopped by a fault alone: refused
    // with the fault, said once, as the CLI says it (exit 2: the command couldn't run),
    // nothing written.
    let dir = Scratch::new("fault-every-body");
    std::fs::write(dir.0.join("part.silico"), SKETCH_AND_BOX).unwrap();
    let out = silico_export_with(
        &dir.0,
        &["part.silico", "part.3mf"],
        Some(Path::new("/nowhere")),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert_eq!(stderr, format!("part.silico: {NO_SKETCH_WORKER}\n"));
    assert!(out.stdout.is_empty());
    assert_eq!(dir.names(), ["part.silico"]);
}

#[test]
fn without_a_tolerance_the_export_is_meshed_at_freecads_0_1_mm() {
    // A cylinder's mesh depends on the tolerance (a box's doesn't), where the tolerance
    // and not FreeCAD's angle sets it: at a radius of 500 mm, an angle of 0.1 rad leaves a
    // chord 500 · (1 - cos 0.05) ≈ 0.62 mm from the arc, past 0.1 but within 1. Exported
    // with none given, its bytes are those at --tolerance=0.1, and more than those at 1.
    let cylinder = BOX.replace(
        r#""type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10"#,
        r#""type": "cylinder", "v": 1, "name": "Block", "position": [0, 0, 0], "radius": 500, "height": 10"#,
    );
    assert_ne!(cylinder, BOX);
    let dir = Scratch::new("default-tolerance");
    std::fs::write(dir.0.join("part.silico"), &cylinder).unwrap();
    let bytes = |args: &[&str]| {
        let out = silico_export(&dir.0, args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let path = dir.0.join(args[args.len() - 1]);
        std::fs::read(path).unwrap()
    };
    let default = bytes(&["part.silico", "default.3mf"]);
    assert!(default == bytes(&["--tolerance=0.1", "part.silico", "fine.3mf"]));
    let coarse = bytes(&["--tolerance=1", "part.silico", "coarse.3mf"]);
    assert!(
        coarse.len() < default.len(),
        "{} {}",
        coarse.len(),
        default.len()
    );
}

#[test]
fn a_name_with_characters_a_terminal_acts_on_is_written_as_it_is_and_said_escaped() {
    // U+009B (a C1 control, the CSI of an escape sequence) and U+202E (right-to-left
    // override): XML 1.0 holds both, so the 3MF names the object with them as they are;
    // stdout escapes them, so they can't act on the terminal. U+0001 can't be held at all:
    // refused, said escaped too, nothing written.
    let dir = Scratch::new("unseen-name");
    let named = BOX.replace(r#""name": "Block""#, r#""name": "Bl\u009bock\u202e""#);
    assert_ne!(named, BOX);
    std::fs::write(dir.0.join("part.silico"), &named).unwrap();
    let out = silico_export(&dir.0, &["part.silico", "part.3mf"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout.lines().nth(1),
        Some("k7f2qx3m \"Bl\\u{9b}ock\\u{202e}\": 12 triangles, 8 vertices, volume 8000 mm³")
    );
    let bytes = std::fs::read(dir.0.join("part.3mf")).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("name=\"Bl\u{9b}ock\u{202e}\""), "{text}");
    let control = BOX.replace(r#""name": "Block""#, r#""name": "Bl\u0001ock""#);
    std::fs::write(dir.0.join("control.silico"), &control).unwrap();
    let out = silico_export(&dir.0, &["control.silico", "control.3mf"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("\\u0001") || stderr.contains("\\u{1}"),
        "{stderr}"
    );
    assert!(!stderr.contains('\u{1}'), "{stderr}");
    assert!(!dir.0.join("control.3mf").exists());
}

#[test]
fn a_step_output_is_written_as_step_each_objects_line_its_id_and_name() {
    // The format is the output's: a .step file holds the box as one product, named by its
    // feature; its line says no mesh numbers, there being no mesh.
    let dir = Scratch::new("step");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    let out = silico_export(&dir.0, &["part.silico", "part.step"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(dir.0.join("part.step")).unwrap();
    assert_eq!(
        stdout,
        format!(
            "wrote part.step ({} bytes)\nk7f2qx3m \"Block\"\n",
            bytes.len()
        )
    );
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("ISO-10303-21;"), "{text}");
    assert!(text.contains("FILE_NAME('part.silico',"), "{text}");
    assert!(text.contains("PRODUCT('k7f2qx3m','Block'"), "{text}");
}

#[test]
fn a_tolerance_with_a_step_output_or_an_output_of_neither_format_is_a_usage_error() {
    let dir = Scratch::new("step-usage");
    std::fs::write(dir.0.join("part.silico"), BOX).unwrap();
    for (args, says) in [
        (
            &["--tolerance=0.1", "part.silico", "part.step"][..],
            "--tolerance is a 3MF mesh's",
        ),
        (
            &["part.silico", "part.stl"][..],
            "must end in .3mf or .step",
        ),
        (&["part.silico", "part"][..], "must end in .3mf or .step"),
    ] {
        let out = silico_export(&dir.0, args);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(says), "{args:?}: {stderr}");
        assert!(stderr.contains("usage: silico"), "{args:?}: {stderr}");
    }
    assert_eq!(dir.names(), ["part.silico"]);
}
