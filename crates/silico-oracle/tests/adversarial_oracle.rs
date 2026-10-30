//! Hostile inputs for the FreeCAD oracle harness (break-it): case files that are corrupt,
//! a FreeCAD that crashes, hangs, or answers garbage, a kernel worker that crashes, command
//! lines that ask for too much or nothing, and pairs of different shapes the comparison
//! should tell apart. Where a test says what the harness did before /break-it found it, it
//! pins the fix. Needs the FreeCAD oracle (`pixi install -e oracle`) and the kernel worker
//! built (`cargo nextest run --workspace` builds it).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/wait.rs"]
mod wait;

use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use silico_core::kernel::{KernelWorker, ShapeSummary};
use silico_oracle::compare::Measured;
use silico_oracle::{
    Case, Chamfer, Outcome, Shape, Tolerance, differences, generate, load, outcomes, run,
};
use wait::WAIT;

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");
const CLI: &str = env!("CARGO_BIN_EXE_silico-oracle");

// ---- Helpers ----

fn case(name: &str, shape: Shape) -> Case {
    Case {
        name: name.to_owned(),
        note: "break-it".to_owned(),
        boundary: None,
        shape,
    }
}

fn block(dx: f64, dy: f64, dz: f64) -> Shape {
    Shape::Block { dx, dy, dz }
}

fn moved(shape: Shape, by: [f64; 3]) -> Shape {
    Shape::Translate {
        shape: Box::new(shape),
        by,
    }
}

fn fillet(shape: Shape, edges: &[u32], radius: f64) -> Shape {
    Shape::Fillet {
        shape: Box::new(shape),
        edges: edges.to_vec(),
        radius,
    }
}

/// A fresh, empty scratch directory for this test, removed when it's dropped.
fn scratch(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("silico-break-oracle-{name}-"))
        .tempdir()
        .unwrap()
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_cases(dir: &Path, cases: &[Case]) {
    std::fs::write(
        dir.join("cases.json"),
        serde_json::to_string(cases).unwrap(),
    )
    .unwrap();
}

/// The real kernel worker, beside the CLI.
fn real_worker() -> PathBuf {
    Path::new(CLI).with_file_name("silico-kernel-worker")
}

/// A `freecadcmd` stand-in that runs the real one with its settings in `home` (empty: a
/// fresh FreeCAD's defaults), so a test neither reads nor rewrites the user's own.
fn hermetic_freecad(dir: &Path, home: &Path) -> PathBuf {
    std::fs::create_dir_all(home).unwrap();
    let real = silico_oracle::freecad::freecadcmd().unwrap();
    let path = dir.join("freecadcmd");
    executable(
        &path,
        &format!(
            "#!/bin/sh\nFREECAD_USER_HOME='{}' exec '{}' \"$@\"\n",
            home.display(),
            real.display()
        ),
    );
    path
}

struct Ran {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
}

/// Runs the CLI with `args` and `env`. A run still going after `limit` is killed, with its
/// children, and has no status.
fn cli(dir: &Path, args: &[&str], env: &[(&str, &Path)], limit: Duration) -> Ran {
    let (out, err) = (dir.join("cli.stdout"), dir.join("cli.stderr"));
    let mut command = Command::new(CLI);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(File::create(&out).unwrap())
        .stderr(File::create(&err).unwrap());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > limit {
            let _ = Command::new("pkill")
                .args(["-KILL", "-P", &child.id().to_string()])
                .status();
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ran {
        status,
        stdout: std::fs::read_to_string(out).unwrap(),
        stderr: std::fs::read_to_string(err).unwrap(),
    }
}

fn summary(kernel: &mut KernelWorker, shape: &Shape) -> ShapeSummary {
    let built = silico_oracle::silico::build(kernel, shape).unwrap();
    kernel.summarize(&built).unwrap()
}

/// What the harness finds different between Silico's outcome for `ours` and FreeCAD's for
/// `theirs`, both built for real, compared as `theirs` is.
fn harness_differences(ours: Shape, theirs: Shape) -> Vec<String> {
    let tolerance = Tolerance::of(&theirs);
    let got = outcomes(&[case("ours", ours), case("theirs", theirs)]).unwrap();
    differences(&got[0].0, &got[1].1, tolerance)
}

/// Every file and directory under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        }
        found.push(path);
    }
    found
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|k| (a[k] - b[k]) * (a[k] - b[k]))
        .sum::<f64>()
        .sqrt()
}

/// Asserts `a` and `b` are different solids (their centers of mass, measured precisely by
/// Silico, at least `apart` mm apart), and returns what the harness makes of Silico's `a`
/// against FreeCAD's `b`.
fn different_solids(a: Shape, b: Shape, apart: f64) -> Vec<String> {
    let mut k = KernelWorker::start().unwrap();
    let (ca, cb) = (summary(&mut k, &a).center, summary(&mut k, &b).center);
    assert!(
        distance(ca, cb) >= apart,
        "the two shapes aren't different: centers {ca:?} and {cb:?}"
    );
    harness_differences(a, b)
}

// ---- Different shapes the comparison tells apart ----

#[test]
fn a_chamfer_flipped_is_told_apart_from_one_not_flipped() {
    // A chamfer of distances 1 and 8 and its flip have the same volume, area, bounding box,
    // and counts (the removed prism is the same triangle mirrored; the box's corners stay),
    // so a Silico whose `flip` did nothing passed the oracle until /break-it found it. Their
    // centers of mass, now compared, are 0.07 mm apart.
    let chamfer = |flip| Shape::Chamfer {
        shape: Box::new(block(10.0, 20.0, 30.0)),
        edges: vec![1],
        size: Chamfer::TwoDistances {
            distance: 1.0,
            other: 8.0,
        },
        flip,
    };
    let found = different_solids(chamfer(false), chamfer(true), 0.05);
    assert!(
        !found.is_empty(),
        "Silico's unflipped chamfer passes as FreeCAD's flipped one"
    );
}

#[test]
fn a_chamfer_distance_and_angle_flipped_is_told_apart() {
    // As above for a distance and an angle: 2 mm and 60° (3.46 mm on the other face)
    // flipped has the same volume, area, box, and counts, and another center.
    let chamfer = |flip| Shape::Chamfer {
        shape: Box::new(block(10.0, 20.0, 30.0)),
        edges: vec![3],
        size: Chamfer::DistanceAngle {
            distance: 2.0,
            angle: 60.0,
        },
        flip,
    };
    let found = different_solids(chamfer(false), chamfer(true), 0.005);
    assert!(
        !found.is_empty(),
        "Silico's unflipped chamfer passes as FreeCAD's flipped one"
    );
}

#[test]
fn a_cone_upside_down_is_told_apart() {
    // A cone 2 at the bottom and 6 at the top, and one 6 at the bottom and 2 at the top,
    // have the same volume (π h (r1² + r1 r2 + r2²) / 3), area, box ([-6, 6]² × [0, 8]), and
    // counts: a Silico that swapped radius1 and radius2 passed every cone case until the
    // center was compared.
    let found = different_solids(
        Shape::Cone {
            radius1: 2.0,
            radius2: 6.0,
            height: 8.0,
        },
        Shape::Cone {
            radius1: 6.0,
            radius2: 2.0,
            height: 8.0,
        },
        1.0,
    );
    assert!(!found.is_empty(), "an upside-down cone passes the oracle");
}

#[test]
fn a_fillet_on_the_opposite_parallel_edge_is_told_apart() {
    // Edge1 of a 10 × 20 × 30 box runs along z at (0, 0), Edge7 along z at (10, 20)
    // (FreeCAD's `Part.makeBox(10, 20, 30).Edges`). Rounding either gives the same volume,
    // area, box, and counts, so until the center was compared the oracle couldn't see
    // Silico numbering parallel edges of one length differently from FreeCAD, which is
    // what the fillet and chamfer cases exist to check.
    let found = different_solids(
        fillet(block(10.0, 20.0, 30.0), &[1], 2.0),
        fillet(block(10.0, 20.0, 30.0), &[7], 2.0),
        0.01,
    );
    assert!(
        !found.is_empty(),
        "a fillet on Edge7 passes as one on Edge1"
    );
}

#[test]
fn a_tiny_fillet_is_told_apart_from_none_and_from_one_ten_times_larger() {
    // `fillet_tiny` rounds Edge1 at 1e-6, near the tolerance boundary. A fillet is told
    // apart from no fillet (it adds a face), and from one ten times larger: the area is
    // 1.2e-4 mm² of 2200 apart (5e-8), past the tolerance for a curved shape with no
    // boolean (1e-8), though not past the one for booleans of curved shapes (1e-5).
    let mut k = KernelWorker::start().unwrap();
    let small = fillet(block(10.0, 20.0, 30.0), &[1], 1e-6);
    let large = fillet(block(10.0, 20.0, 30.0), &[1], 1e-5);
    let (a, b) = (summary(&mut k, &small).area, summary(&mut k, &large).area);
    // −(2 − π/2) r L: 1.29e-5 and 1.29e-4 below the box's 2200.
    assert!((a - b) > 1e-4, "{a} {b}");
    let found = harness_differences(small.clone(), large);
    assert!(
        found.iter().any(|d| d.starts_with("area")),
        "a fillet of 1e-5 passes as one of 1e-6: {found:?}"
    );
    let found = harness_differences(small, block(10.0, 20.0, 30.0));
    assert!(
        found.iter().any(|d| d.starts_with("faces")),
        "a tiny fillet passes as no fillet: {found:?}"
    );
}

#[test]
fn a_chamfer_a_little_too_deep_is_told_apart() {
    // Edge1 of a 10 × 20 × 30 box (30 long) chamfered at 2.0008 for 2: 0.048 mm³ more cut
    // away, 8e-6 of the volume. A shape of planes is compared to 1e-9.
    let chamfer = |distance| Shape::Chamfer {
        shape: Box::new(block(10.0, 20.0, 30.0)),
        edges: vec![1],
        size: Chamfer::Equal { distance },
        flip: false,
    };
    let found = harness_differences(chamfer(2.0008), chamfer(2.0));
    assert!(
        found.iter().any(|d| d.starts_with("volume")),
        "a chamfer 0.04% too deep passes: {found:?}"
    );
}

#[test]
fn a_box_whose_sides_went_through_f32_is_told_apart() {
    // 0.1, 3.3, and 7.07 narrowed to 32-bit floats and back: 1.5e-8 of a side off.
    let narrowed = block(f64::from(0.1f32), f64::from(3.3f32), f64::from(7.07f32));
    let found = harness_differences(narrowed, block(0.1, 3.3, 7.07));
    assert!(!found.is_empty(), "a box of f32 sides passes");
}

// ---- The FreeCAD side builds what the case says ----

#[test]
fn chamfer_of_a_fused_shape_matches_freecad_unrefined() {
    // oracle.py chamfers through `PartDesign::Chamfer`, whose `Refine` defaults to FreeCAD's
    // `Mod/PartDesign/RefineModel` setting, true out of the box
    // (PartDesign/App/FeatureRefine.cpp:70). Silico doesn't refine yet, and
    // docs/ports/booleans.md says its results match FreeCAD's with `Refine = False`, which
    // oracle.py didn't set until /break-it found it: two unit-10 cubes fused side by side,
    // Edge1 chamfered at 1, were 11 faces in Silico and 7 in FreeCAD (refined).
    let temp = scratch("refine");
    let dir = temp.path();
    let freecad = hermetic_freecad(dir, &dir.join("home"));
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let fused = Shape::Fuse {
        base: Box::new(block(10.0, 10.0, 10.0)),
        tool: Box::new(moved(block(10.0, 10.0, 10.0), [10.0, 0.0, 0.0])),
    };
    write_cases(
        &cases,
        &[case(
            "chamfer_on_fused",
            Shape::Chamfer {
                shape: Box::new(fused),
                edges: vec![1],
                size: Chamfer::Equal { distance: 1.0 },
                flip: false,
            },
        )],
    );
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        Duration::from_secs(90),
    );
    assert_eq!(
        ran.stdout, "0 of 1 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
}

#[test]
fn the_oracle_verdict_doesnt_depend_on_the_users_freecad_settings() {
    // The oracle's FreeCAD read, and on exit rewrote, the user's own FreeCAD settings (on
    // macOS ~/Library/Preferences/FreeCAD/v1-1/user.cfg, the GUI's), until /break-it found
    // it: unticking "Automatically refine model after sketch-based operation" in FreeCAD's
    // Part Design preferences turned a difference into a match. Now it has settings of its
    // own, and the cases pin what they change. Here: the same case under FreeCAD's
    // defaults and under RefineModel = false.
    let temp = scratch("settings");
    let dir = temp.path();
    let (default, unrefined) = (dir.join("home-default"), dir.join("home-unrefined"));
    let set = dir.join("set.py");
    std::fs::write(
        &set,
        "import FreeCAD\nFreeCAD.ParamGet('User parameter:BaseApp/Preferences/Mod/PartDesign')\
         .SetBool('RefineModel', False)\n",
    )
    .unwrap();
    std::fs::create_dir_all(&unrefined).unwrap();
    let status = Command::new(silico_oracle::freecad::freecadcmd().unwrap())
        .arg(&set)
        .env("FREECAD_USER_HOME", &unrefined)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let config = std::fs::read_to_string(unrefined.join("user.cfg")).unwrap();
    assert!(
        config.contains(r#"<FCBool Name="RefineModel" Value="0"/>"#),
        "the setting wasn't saved"
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let fused = Shape::Fuse {
        base: Box::new(block(10.0, 10.0, 10.0)),
        tool: Box::new(moved(block(10.0, 10.0, 10.0), [10.0, 0.0, 0.0])),
    };
    write_cases(
        &cases,
        &[case(
            "chamfer_on_fused",
            Shape::Chamfer {
                shape: Box::new(fused),
                edges: vec![1],
                size: Chamfer::Equal { distance: 1.0 },
                flip: false,
            },
        )],
    );
    let verdict = |name: &str, home: &Path| {
        let freecad = hermetic_freecad(&dir.join(name), home);
        cli(
            dir,
            &["run", cases.to_str().unwrap()],
            &[("SILICO_FREECADCMD", &freecad)],
            Duration::from_secs(90),
        )
        .stdout
    };
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    assert_eq!(verdict("a", &default), verdict("b", &unrefined));
}

#[test]
fn fillet_of_edge_0_is_refused_on_both_sides() {
    // Edges are numbered from 1, and Silico refuses Edge0 ("edge 0 doesn't exist"), as
    // FreeCAD itself does ("Invalid edge link"). oracle.py filleted `s.Edges[i - 1]`, and
    // Python reads index −1 as the last edge, so FreeCAD's side rounded Edge12 until
    // /break-it found it.
    let cases = [case(
        "fillet_edge0",
        fillet(block(10.0, 20.0, 30.0), &[0], 1.0),
    )];
    let (ours, theirs) = outcomes(&cases).unwrap().remove(0);
    assert!(matches!(ours, Outcome::Failed(_)), "{ours:?}");
    assert!(
        matches!(theirs, Outcome::Failed(_)),
        "FreeCAD's side built Edge0: {theirs:?}"
    );
}

#[test]
fn extrude_along_a_direction_whose_length_overflows_matches_freecad() {
    // Silico extrudes the rectangle along [1e200, 0, 1e200] (the direction [1, 0, 1]) to a
    // 10 × 20 × 5 prism leaning 45°, as FreeCAD does given the vector [5, 0, 5]. oracle.py
    // normalized the direction by sqrt(x² + y² + z²), which overflows, and reported its own
    // "division by zero" as FreeCAD's failure until /break-it found it.
    let cases = [case(
        "extrude_huge_direction",
        Shape::Extrude {
            profile: vec![
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 20.0, 0.0],
                [0.0, 20.0, 0.0],
            ],
            length: 5.0,
            direction: Some([1e200, 0.0, 1e200]),
        },
    )];
    let (ours, theirs) = outcomes(&cases).unwrap().remove(0);
    let Outcome::Built(m) = &ours else {
        panic!("{ours:?}")
    };
    assert!((m.volume - 1000.0).abs() < 1e-6, "{m:?}");
    let found = differences(&ours, &theirs, Tolerance::of(&cases[0].shape));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_oracles_freecad_never_touches_the_users_settings() {
    // FreeCAD keeps its settings in the user's home: on macOS where the system says it
    // is, which CFFIXED_USER_HOME overrides, elsewhere HOME or XDG_CONFIG_HOME. Here they
    // all name a scratch directory, and a run of a case leaves nothing of FreeCAD's there:
    // the harness gives FreeCAD settings of its own.
    let temp = scratch("user-home");
    let dir = temp.path();
    let home = dir.join("user");
    std::fs::create_dir_all(&home).unwrap();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let config = home.join(".config");
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("HOME", &home),
            ("CFFIXED_USER_HOME", &home),
            ("XDG_CONFIG_HOME", &config),
        ],
        Duration::from_secs(90),
    );
    assert_eq!(
        ran.stdout, "0 of 1 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    let written: Vec<PathBuf> = files(&home)
        .into_iter()
        .filter(|p| p.to_string_lossy().to_lowercase().contains("freecad"))
        .collect();
    assert!(written.is_empty(), "FreeCAD wrote {written:?}");
}

// ---- A hang, and a crash of Silico's worker ----

#[test]
fn hung_freecad_oracle_times_out() {
    // A FreeCAD that hangs on a case (OCCT has infinite loops) is killed after the case
    // deadline, and the case is its failure, as a crash is; the harness goes on. It waited
    // forever, with `Command::output`, until /break-it found it. Here a stand-in starts the
    // cases and never answers, and the deadline is two seconds.
    let temp = scratch("hang");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\nexec sleep 400\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
        ],
        WAIT,
    );
    assert!(
        ran.status.is_some(),
        "the harness was still waiting for FreeCAD after {WAIT:?}"
    );
    assert!(
        ran.stdout
            .contains("FreeCAD took longer than 2 s building it"),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_freecad_that_keeps_printing_but_never_finishes_a_case_is_killed_at_the_deadline() {
    // Only an outcome restarts the deadline: FreeCAD's own output doesn't, or a FreeCAD
    // that hangs printing progress would never be killed. Here a stand-in starts the cases,
    // then prints a line a second.
    let temp = scratch("hang-chatty");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\n\
         while :; do echo 'Recompute: still going'; sleep 1; done\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
        ],
        WAIT,
    );
    assert!(
        ran.status.is_some(),
        "still waiting after {WAIT:?}, with a deadline of 2"
    );
    assert!(
        ran.stdout.contains(
            "box: Silico built it; FreeCAD failed: FreeCAD took longer than 2 s building it; \
             its last output:\nRecompute: still going"
        ),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_freecad_that_floods_its_output_is_killed_at_the_deadline() {
    // Lines waiting to be read don't hold the deadline off, and can't pile up in memory:
    // FreeCAD blocks writing once a backlog waits.
    let temp = scratch("flood");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\nexec yes 'Recompute: still going'\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
        ],
        WAIT,
    );
    assert!(
        ran.status.is_some(),
        "still waiting after {WAIT:?}, with a deadline of 2"
    );
    assert!(
        ran.stdout
            .contains("FreeCAD took longer than 2 s building it"),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_freecad_that_writes_text_that_isnt_utf8_then_goes_quiet_is_killed_at_the_deadline() {
    // Its output is read as bytes, so a line that isn't UTF-8 neither ends the reading
    // (which ended the wait's deadline with it) nor is lost from the report.
    let temp = scratch("bad-utf8");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\nprintf 'caf\\351\\n'\nexec sleep 400\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
        ],
        WAIT,
    );
    assert!(
        ran.status.is_some(),
        "still waiting after {WAIT:?}, with a deadline of 2"
    );
    assert!(
        ran.stdout
            .contains("FreeCAD took longer than 2 s building it; its last output:\ncaf\u{fffd}\n"),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_freecad_that_closes_its_output_but_keeps_running_is_killed_at_the_deadline() {
    // Its output closing isn't taken for its end: it's given until its deadline.
    let temp = scratch("closed-output");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\nexec sleep 400 >&- 2>&-\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
        ],
        WAIT,
    );
    assert!(
        ran.status.is_some(),
        "still waiting after {WAIT:?}, with a deadline of 2"
    );
    assert!(
        ran.stdout.contains(
            "box: Silico built it; FreeCAD failed: FreeCAD took longer than 2 s building it\n"
        ),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_crash_report_keeps_the_last_20_lines_each_cut_to_1000_bytes() {
    // 25 lines, then one of 'a' and 1500 'é' (3001 bytes, byte 1000 inside an 'é'), then
    // a crash: the report keeps lines 7 to 25, and the long line cut to 999 bytes.
    let temp = scratch("tail");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    let long = format!("a{}", "é".repeat(1500));
    let mut body = String::from("#!/bin/sh\necho SILICO_ORACLE_READY\n");
    for n in 1..=25 {
        body.push_str(&format!("echo 'line {n}'\n"));
    }
    body.push_str(&format!("echo '{long}' >&2\nexit 3\n"));
    executable(&freecad, &body);
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        WAIT,
    );
    let kept: String = (7..=25).map(|n| format!("line {n}\n")).collect();
    let cut = format!("a{}", "é".repeat(499));
    assert_eq!(cut.len(), 999);
    assert_eq!(
        ran.stdout,
        format!(
            "box: Silico built it; FreeCAD failed: FreeCAD crashed building it (exit status: 3); \
             its last output:\n{kept}{cut}\n1 of 1 cases differ from FreeCAD\n"
        ),
        "{}",
        ran.stderr
    );
}

#[test]
fn each_answer_restarts_freecads_deadline() {
    // Six cases, each answered 1 s after the last, with a deadline of 4: the run takes
    // longer than the deadline, but no case comes near it (3 s to spare on a loaded machine).
    let temp = scratch("deadline-restarts");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    let answer = "sleep 1; echo 'SILICO_ORACLE_CASE {\"failed\": \"stand-in\"}'\n";
    executable(
        &freecad,
        &format!("#!/bin/sh\necho SILICO_ORACLE_READY\n{}", answer.repeat(6)),
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let all: Vec<Case> = (0..6)
        .map(|n| case(&format!("box_{n}"), block(1.0, 2.0, 3.0)))
        .collect();
    write_cases(&cases, &all);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_ORACLE_CASE_DEADLINE", Path::new("4")),
        ],
        WAIT,
    );
    let want: String = (0..6)
        .map(|n| format!("box_{n}: Silico built it; FreeCAD failed: stand-in\n"))
        .chain(["6 of 6 cases differ from FreeCAD\n".to_owned()])
        .collect();
    assert_eq!(ran.stdout, want, "{}", ran.stderr);
}

#[test]
fn a_run_compares_each_case_at_its_shapes_tolerance() {
    // A stand-in answers a 10 × 20 × 30 box with a volume 5e-9 too large: past the 1e-9 a
    // shape of planes is compared to, though not the 1e-5 of a boolean of curved shapes.
    let temp = scratch("run-planar");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho SILICO_ORACLE_READY\n\
         echo 'SILICO_ORACLE_CASE {\"built\": {\"volume\": 6000.00003, \"area\": 2200.0, \
         \"bounds\": [[0.0, 0.0, 0.0], [10.0, 20.0, 30.0]], \"center\": [5.0, 10.0, 15.0], \
         \"faces\": 6, \"edges\": 12, \"vertices\": 8, \"valid\": true}}'\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(10.0, 20.0, 30.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        WAIT,
    );
    assert!(
        ran.stdout.starts_with("box: volume: Silico 6000")
            && ran.stdout.ends_with("1 of 1 cases differ from FreeCAD\n"),
        "{}{}",
        ran.stdout,
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1));
}

#[test]
fn a_case_both_sides_refuse_is_named_as_refused() {
    // Edge0 doesn't exist on either side: they agree, but nothing was compared, so the
    // case is named.
    let edge0 = fillet(block(10.0, 20.0, 30.0), &[0], 1.0);
    let report = run(&[case("fillet_edge0", edge0.clone())]).unwrap();
    assert!(report.differences.is_empty(), "{:?}", report.differences);
    assert_eq!(report.refused, ["fillet_edge0"]);
    let temp = scratch("refused");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("fillet_edge0", edge0)]);
    let ran = cli(dir, &["run", cases.to_str().unwrap()], &[], WAIT);
    assert_eq!(
        ran.stdout, "0 of 1 cases differ from FreeCAD; both refused 1: fillet_edge0\n",
        "{}",
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(0));
}

#[test]
fn a_curved_boolean_a_little_too_large_is_told_apart() {
    // A sphere of radius 10 in common with a box that cuts a cap off it, against the same
    // with the radius 2e-5 larger: about 6e-5 apart in volume, past the 1e-5 a boolean of
    // curved shapes is compared to.
    let common = |radius| Shape::Common {
        base: Box::new(Shape::Sphere { radius }),
        tool: Box::new(moved(block(30.0, 30.0, 30.0), [-5.0, -15.0, -15.0])),
    };
    assert_eq!(Tolerance::of(&common(10.0)), Tolerance::CURVED_BOOLEAN);
    let found = harness_differences(common(10.0002), common(10.0));
    assert!(
        found.iter().any(|d| d.starts_with("volume")),
        "a curved boolean 6e-5 too large passes: {found:?}"
    );
}

#[test]
fn a_line_past_a_megabyte_is_cut_and_its_rest_dropped() {
    // The rest of a line too long to keep never arrives as a line of its own, so a marker
    // inside it isn't taken for an outcome.
    let temp = scratch("long-line");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho SILICO_ORACLE_READY\n\
         { head -c 1048586 /dev/zero | tr '\\0' x; \
         echo 'SILICO_ORACLE_CASE {\"failed\": \"injected\"}'; }\nexit 3\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        WAIT,
    );
    assert!(
        ran.stdout.starts_with(
            "box: Silico built it; FreeCAD failed: FreeCAD crashed building it (exit status: 3)"
        ),
        "{}",
        &ran.stdout[..ran.stdout.len().min(300)]
    );
    assert!(!ran.stdout.contains("injected"));
}

#[test]
fn a_freecad_that_crashes_or_hangs_on_a_case_silico_refuses_is_a_difference() {
    // Edge0 doesn't exist: Silico refuses it. FreeCAD crashing, hanging, or stopping on it
    // compared nothing, so the run fails, saying how FreeCAD ended.
    let temp = scratch("freecad-lost-vs-refusal");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(
        &cases,
        &[case(
            "fillet_edge0",
            fillet(block(10.0, 20.0, 30.0), &[0], 1.0),
        )],
    );
    for (name, body, says) in [
        (
            "crashes",
            "exit 139",
            "FreeCAD crashed building it (exit status: 139)",
        ),
        (
            "hangs",
            "exec sleep 400",
            "FreeCAD took longer than 2 s building it",
        ),
        (
            "stops",
            "exit 0",
            "FreeCAD stopped building it (exit status: 0)",
        ),
    ] {
        let freecad = dir.join(name);
        executable(
            &freecad,
            &format!("#!/bin/sh\necho SILICO_ORACLE_READY\n{body}\n"),
        );
        let ran = cli(
            dir,
            &["run", cases.to_str().unwrap()],
            &[
                ("SILICO_FREECADCMD", &freecad),
                ("SILICO_ORACLE_CASE_DEADLINE", Path::new("2")),
            ],
            WAIT,
        );
        assert_eq!(
            ran.status.and_then(|s| s.code()),
            Some(1),
            "{name}: {}",
            ran.stdout
        );
        assert!(
            ran.stdout
                .starts_with("fillet_edge0: Silico failed: invalid_input: ")
                && ran.stdout.contains(&format!("; FreeCAD failed: {says}\n")),
            "{name}: {}",
            ran.stdout
        );
    }
}

#[test]
fn a_crash_of_silicos_worker_on_a_case_freecad_refuses_is_a_difference() {
    // Both failing agrees only when both refused: a side that crashed or hung compared
    // nothing. (Its worker kills itself as it starts, before its first frame: one that
    // can't be had, worker_unavailable, lost as a crash is.)
    let temp = scratch("crash-vs-refusal");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho\necho SILICO_ORACLE_READY\n\
         echo 'SILICO_ORACLE_CASE {\"failed\": \"FreeCAD refused it\"}'\n",
    );
    let worker = dir.join("worker");
    executable(&worker, "#!/bin/sh\nkill -KILL $$\n");
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_KERNEL_WORKER", &worker),
        ],
        WAIT,
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1), "{}", ran.stdout);
    assert!(
        ran.stdout
            .starts_with("box: Silico failed: worker_unavailable: ")
            && ran.stdout.ends_with(
                "; FreeCAD failed: FreeCAD refused it\n1 of 1 cases differ from FreeCAD\n"
            ),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_freecad_crash_is_reported_with_how_it_ended_and_what_it_last_wrote() {
    // As the worker client reports a worker's crash: its exit status and its last output,
    // in the order it wrote it (its stdout and stderr are one pipe: the stand-in checks).
    let temp = scratch("crash-report");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\n[ /dev/fd/1 -ef /dev/fd/2 ] || exit 7\necho\necho SILICO_ORACLE_READY\n\
         echo 'Program received signal SIGSEGV, Segmentation fault.' >&2\n\
         echo '#0  BRepFilletAPI_MakeFillet::Build()' >&2\nexit 139\n",
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        WAIT,
    );
    assert_eq!(
        ran.stdout,
        "box: Silico built it; FreeCAD failed: FreeCAD crashed building it (exit status: 139); \
         its last output:\nProgram received signal SIGSEGV, Segmentation fault.\n\
         #0  BRepFilletAPI_MakeFillet::Build()\n1 of 1 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
}

#[test]
fn a_case_after_one_that_crashes_silicos_worker_still_matches() {
    // Silico's side starts a new kernel worker after a crash or a timeout, as FreeCAD's
    // side starts a new FreeCAD. It built every case on one, stopped for good after a
    // failure, until /break-it found it: every later case failed with "this kernel worker
    // was stopped after an earlier failure". Here the worker crashes once, on the first
    // case; the second, a plain box, matches.
    let temp = scratch("worker-crash");
    let dir = temp.path();
    let freecad = hermetic_freecad(dir, &dir.join("home"));
    let worker = dir.join("worker");
    let mark = dir.join("crashed-once");
    executable(
        &worker,
        &format!(
            "#!/bin/sh\nif [ ! -e '{mark}' ]; then : > '{mark}'; kill -KILL $$; fi\nexec '{real}'\n",
            mark = mark.display(),
            real = real_worker().display()
        ),
    );
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(
        &cases,
        &[
            case("first", block(1.0, 2.0, 3.0)),
            case("second", block(4.0, 5.0, 6.0)),
        ],
    );
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[
            ("SILICO_FREECADCMD", &freecad),
            ("SILICO_KERNEL_WORKER", &worker),
        ],
        Duration::from_secs(90),
    );
    assert!(
        ran.stdout
            .contains("first: FreeCAD built it; Silico failed"),
        "the worker didn't crash on the first case: {}{}",
        ran.stdout,
        ran.stderr
    );
    assert!(
        !ran.stdout.contains("second:"),
        "the case after the crash: {}",
        ran.stdout
    );
}

// ---- The command line ----

#[test]
fn sweep_of_more_cases_than_memory_holds_is_refused_not_a_crash() {
    // `generate::cases` collects `count` cases into a Vec sized up front, so before
    // /break-it `--count 18446744073709551615` panicked ("capacity overflow", exit 101) and
    // `--count 1000000000000000` aborted ("memory allocation of 152000000000000000 bytes
    // failed", exit 134). Now: the usage error, exit 2, as for any bad --count.
    let temp = scratch("huge-count");
    let dir = temp.path();
    for count in ["18446744073709551615", "1000000000000000"] {
        let ran = cli(dir, &["sweep", "--count", count], &[], WAIT);
        assert_eq!(
            ran.status.and_then(|s| s.code()),
            Some(2),
            "--count {count}: {}",
            ran.stderr
        );
        assert!(
            ran.stderr.starts_with(&format!(
                "silico-oracle: --count {count}: at most 1000000\n"
            )),
            "{}",
            ran.stderr
        );
    }
}

#[test]
fn a_run_of_no_cases_is_an_error_not_a_pass() {
    // A directory with no `.json` files directly in it (empty, a typo'd subfolder, cases
    // saved as `.JSON` or one level down) and `sweep --count 0` printed "0 of 0 cases
    // differ from FreeCAD" and exited 0 before /break-it: nothing compared, reading as a
    // pass, which `freecad.rs` rules out for a missing FreeCAD.
    let temp = scratch("no-cases");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(cases.join("nested")).unwrap();
    std::fs::write(cases.join("upper.JSON"), "[]").unwrap();
    write_cases(&cases.join("nested"), &[case("box", block(1.0, 2.0, 3.0))]);
    let ran = cli(dir, &["run", cases.to_str().unwrap()], &[], WAIT);
    let refused = "silico-oracle: there are no cases to compare\n";
    assert_eq!(ran.status.and_then(|s| s.code()), Some(2), "{}", ran.stdout);
    assert_eq!(ran.stderr, refused);
    let ran = cli(dir, &["sweep", "--count", "0"], &[], WAIT);
    assert_eq!(ran.status.and_then(|s| s.code()), Some(2), "{}", ran.stdout);
    assert_eq!(ran.stderr, refused);
}

#[test]
fn a_sweep_runs_the_cases_of_its_seed_and_count() {
    // Against a FreeCAD stand-in that fails every case, each case Silico builds is a
    // difference, named for its seed and number: `--count 2 --seed 5` runs exactly
    // generated_5_0 and generated_5_1. Against FreeCAD they match.
    let temp = scratch("sweep");
    let dir = temp.path();
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        "#!/bin/sh\necho SILICO_ORACLE_READY\n\
         echo 'SILICO_ORACLE_CASE {\"failed\": \"stand-in\"}'\n\
         echo 'SILICO_ORACLE_CASE {\"failed\": \"stand-in\"}'\n",
    );
    let args = ["sweep", "--count", "2", "--seed", "5"];
    let ran = cli(dir, &args, &[("SILICO_FREECADCMD", &freecad)], WAIT);
    assert_eq!(
        ran.stdout,
        "generated_5_0: Silico built it; FreeCAD failed: stand-in\n\
         generated_5_1: Silico built it; FreeCAD failed: stand-in\n\
         2 of 2 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    let ran = cli(dir, &args, &[], Duration::from_secs(90));
    assert_eq!(
        ran.stdout, "0 of 2 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(0));
}

#[test]
fn a_run_of_more_than_one_directory_is_refused() {
    // `pixi run oracle` names tests/oracle; a directory after it was ignored, and the run
    // passed on cases the user didn't ask for.
    let temp = scratch("two-dirs");
    let dir = temp.path();
    let ran = cli(dir, &["run", CASES, "/nonexistent"], &[], WAIT);
    assert_eq!(ran.status.and_then(|s| s.code()), Some(2), "{}", ran.stdout);
    assert!(
        ran.stderr
            .starts_with("silico-oracle: run takes one directory\n"),
        "{}",
        ran.stderr
    );
}

#[test]
fn a_corpus_of_no_step_files_is_an_error_not_a_pass() {
    // `corpus DIR` reads DIR's STEP files; with none there is nothing to compare, the error
    // a run of no cases is, not the usage error of a corpus without its one directory.
    let temp = scratch("empty-corpus");
    let dir = temp.path();
    let corpus = dir.join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();
    let ran = cli(dir, &["corpus", corpus.to_str().unwrap()], &[], WAIT);
    assert_eq!(ran.status.and_then(|s| s.code()), Some(2), "{}", ran.stdout);
    assert_eq!(ran.stderr, "silico-oracle: there are no cases to compare\n");
}

/// A document of `features` (JSON), as the oracle's `.silico` case files are.
fn document(features: &str) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{features}]}}"
    )
}

#[test]
fn a_directory_of_sketch_cases_alone_is_run_and_counted() {
    // No shape cases and no sketch-feature cases: the one sketch case is solved by both
    // sides and is the run's one case, not a run of no cases.
    let temp = scratch("sketches-alone");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let line = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "free_line", "plane": "xy",
      "points": [{"id": "p0000001", "x": 1, "y": 2, "construction": false}, {"id": "p0000002", "x": 11, "y": 3, "construction": false}],
      "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}],
      "circles": [], "arcs": [], "constraints": [], "note": "A line with no constraints."}"#;
    std::fs::write(cases.join("sketches.silico"), document(line)).unwrap();
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[],
        Duration::from_secs(120),
    );
    assert_eq!(
        ran.stdout, "0 of 1 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(0));
}

#[test]
fn a_directory_of_sketch_feature_cases_alone_is_run_and_its_differences_counted() {
    // No shape cases and no sketch cases: the unit square padded 1 (volume 1, FreeCAD's
    // testPartDesignElementMapPad), said to be expected at volume 2, is the run's one case,
    // and it differs, Silico's volume from the expected, so the run fails.
    let temp = scratch("features-alone");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let pad = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "unit_square", "plane": "xy",
      "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 1, "y": 0, "construction": false}, {"id": "p0000003", "x": 1, "y": 1, "construction": false}, {"id": "p0000004", "x": 0, "y": 1, "construction": false}],
      "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000003", "to": "p0000004", "construction": false}, {"id": "n0000004", "from": "p0000004", "to": "p0000001", "construction": false}],
      "circles": [], "arcs": [], "constraints": []},
      {"id": "p0000005", "type": "pad", "v": 1, "name": "unit_square_pad", "sketch": "s0000001",
      "extent": {"type": "one_side", "length": 1, "reversed": false},
      "note": "The unit square padded 1. Expected: volume 2."}"#;
    std::fs::write(cases.join("sketch-features.silico"), document(pad)).unwrap();
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[],
        Duration::from_secs(120),
    );
    let lines: Vec<&str> = ran.stdout.lines().collect();
    let volume = lines.first().and_then(|l| {
        l.strip_prefix("unit_square_pad: Silico's volume ")?
            .strip_suffix(" against the expected 2")?
            .parse::<f64>()
            .ok()
    });
    assert!(
        volume.is_some_and(|v| (v - 1.0).abs() < 1e-9)
            && lines.get(1) == Some(&"1 of 1 cases differ from FreeCAD")
            && lines.len() == 2,
        "{}{}",
        ran.stdout,
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1));
}

#[test]
fn a_directory_of_measure_cases_alone_is_run_and_its_differences_counted() {
    // No other cases: a 10 mm cube's bottom and top, 10 apart, said to be 11 apart, is the
    // run's one case. FreeCAD's MeasureDistance agrees with Silico's 10, so the one
    // difference is Silico's distance from the case's, and the run fails.
    let temp = scratch("measures-alone");
    let dir = temp.path();
    let measures = dir.join("cases").join(silico_oracle::measures::DIR);
    std::fs::create_dir_all(&measures).unwrap();
    std::fs::write(
        measures.join(silico_oracle::measures::MODEL),
        r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bxa00001", "type": "box", "v": 1, "name": "cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}]}"#,
    )
    .unwrap();
    std::fs::write(
        measures.join(silico_oracle::measures::CASES),
        r#"[{"name": "cube_bottom_and_top", "note": "10 apart, said to be 11.", "elements": ["bxa00001.box.Face5", "bxa00001.box.Face6"],
            "expected": {"distance": {"distance": 11, "center_distance": null, "normal_distance": 10}}}]"#,
    )
    .unwrap();
    let cases = dir.join("cases");
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[],
        Duration::from_secs(120),
    );
    assert_eq!(
        ran.stdout,
        "cube_bottom_and_top: Silico's distance 10 against the expected 11\n\
         1 of 1 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1));
}

#[test]
fn a_run_of_measure_cases_prints_each_that_differs_and_counts_them_all() {
    // Two pairs of a 10 mm cube's bottom and top, each 10 apart as the case says, against a
    // FreeCAD stand-in that measures the first 10 apart and the second 11: the second alone
    // is printed, with how it differs, and counted, of the run's two cases.
    let temp = scratch("measures-counted");
    let dir = temp.path();
    let measures = dir.join("cases").join(silico_oracle::measures::DIR);
    std::fs::create_dir_all(&measures).unwrap();
    std::fs::write(
        measures.join(silico_oracle::measures::MODEL),
        r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bxa00001", "type": "box", "v": 1, "name": "cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}]}"#,
    )
    .unwrap();
    let case = |name: &str| {
        format!(
            r#"{{"name": "{name}", "note": "", "elements": ["bxa00001.box.Face5", "bxa00001.box.Face6"],
                "expected": {{"distance": {{"distance": 10, "center_distance": null, "normal_distance": 10}}}}}}"#
        )
    };
    std::fs::write(
        measures.join(silico_oracle::measures::CASES),
        format!("[{}, {}]", case("agrees"), case("differs")),
    )
    .unwrap();
    let measured = |distance: f64| {
        format!(
            "echo 'SILICO_ORACLE_CASE {{\"measured\": {{\"elements\": [\"Face5\", \"Face6\"], \
             \"distance\": {distance}, \"from\": [0.0, 0.0, 0.0], \"to\": [0.0, 0.0, {distance}], \
             \"dist_to_shape\": {distance}, \"witness\": [0.0, 0.0]}}}}'\n"
        )
    };
    let freecad = dir.join("freecadcmd");
    executable(
        &freecad,
        &format!(
            "#!/bin/sh\necho SILICO_ORACLE_READY\n{}{}",
            measured(10.0),
            measured(11.0)
        ),
    );
    let cases = dir.join("cases");
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        Duration::from_secs(120),
    );
    assert_eq!(
        ran.stdout,
        "differs: Silico's distance (FreeCAD's MeasureDistance) 10 against FreeCAD's Some(11.0)\n\
         1 of 2 cases differ from FreeCAD\n",
        "{}",
        ran.stderr
    );
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1));
}

// ---- Generated cases from other seeds ----

#[test]
fn generated_cases_from_other_seeds_match_freecad() {
    // The 3 of 6000 generated cases (seeds 2 to 7, 1000 each) that differed when /break-it
    // ran, all booleans of curved shapes: seed 3 #426 (a torus common a box, volume 1.1e-7
    // apart), seed 6 #147 (a torus fused with a cone, 1.0e-7 in volume, 1.2e-7 in area),
    // seed 7 #653 (a sphere common a torus: volume 1.2e-6 apart, the box's lowest z 0.0103
    // mm apart). The shapes agreed; the measures didn't. The box was BRepBndLib's loose
    // one, below the torus's true bottom (−5.756) by 0.019 on one side and 0.008 on the
    // other, and is now the tight one. The volume is OCCT's default integration, whose
    // error depends on how the trimming curves are parametrized (FreeCAD's booleans run in
    // parallel, Silico's in sequence), and its tolerance is now calibrated on 7000 cases,
    // not 299.
    let mut cases = Vec::new();
    for (seed, index) in [(3, 426), (6, 147), (7, 653)] {
        cases.push(generate::cases(seed, index + 1).remove(index));
    }
    let found = run(&cases).unwrap();
    assert!(found.refused.is_empty(), "{:?}", found.refused);
    assert!(
        found.differences.is_empty(),
        "{}",
        found
            .differences
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn generated_extrude_directions_are_within_45_degrees_of_the_normal() {
    // `generate::extrude` sweeps "up a direction less than 45° from" the normal (+z).
    // It drew [−1..1, −1..1, 1], up to 54.7° from it, until /break-it found it.
    let mut steepest = (0.0, [0.0; 3]);
    for seed in 0..200 {
        for c in generate::cases(seed, 50) {
            if let Shape::Extrude {
                direction: Some([x, y, z]),
                ..
            } = c.shape
            {
                let tan_squared = (x * x + y * y) / (z * z);
                if tan_squared > steepest.0 {
                    steepest = (tan_squared, [x, y, z]);
                }
            }
        }
    }
    assert!(
        steepest.0 <= 1.0,
        "{:?} is more than 45° from the normal (tan² {})",
        steepest.1,
        steepest.0
    );
}

// ---- FreeCAD crashing, failing to start, or answering garbage ----

/// A FreeCAD stand-in that runs the real one and kills it (SIGKILL, as a segfault would end
/// it) as it starts any case that's a sphere of radius 13.
fn freecad_killed_on_sphere_13(dir: &Path) -> PathBuf {
    let hook = dir.join("hook.py");
    std::fs::write(
        &hook,
        r#"import json, os, signal
import FreeCAD
_cases = json.load(open(os.environ["SILICO_ORACLE_CASES"], encoding="utf-8"))[int(os.environ["SILICO_ORACLE_FIRST"]):]
_seen = [0]
_new = FreeCAD.newDocument
def _hooked(*a, **k):
    i = _seen[0]
    _seen[0] += 1
    if _cases[i]["shape"] == {"sphere": {"radius": 13.0}}:
        os.kill(os.getpid(), signal.SIGKILL)
    return _new(*a, **k)
FreeCAD.newDocument = _hooked
_path = os.environ["BREAK_IT_ORACLE_SCRIPT"]
exec(compile(open(_path, encoding="utf-8").read(), _path, "exec"))
"#,
    )
    .unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let path = dir.join("freecadcmd");
    executable(
        &path,
        &format!(
            "#!/bin/sh\nBREAK_IT_ORACLE_SCRIPT=\"$1\" FREECAD_USER_HOME='{}' exec '{}' '{}'\n",
            home.display(),
            silico_oracle::freecad::freecadcmd().unwrap().display(),
            hook.display()
        ),
    );
    path
}

#[test]
fn freecad_oracle_crashing_on_some_cases_loses_only_those() {
    // FreeCAD killed on the first case, the last, and two in a row in between: each is
    // FreeCAD's failure (a difference, as Silico builds it), and every other case is still
    // compared, in order.
    let temp = scratch("freecad-crash");
    let dir = temp.path();
    let freecad = freecad_killed_on_sphere_13(dir);
    let crash = || Shape::Sphere { radius: 13.0 };
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(
        &cases,
        &[
            case("crash_1", crash()),
            case("box", block(1.0, 2.0, 3.0)),
            case("crash_2", crash()),
            case("crash_3", crash()),
            case(
                "cylinder",
                Shape::Cylinder {
                    radius: 2.0,
                    height: 5.0,
                },
            ),
            case("crash_4", crash()),
        ],
    );
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        Duration::from_secs(120),
    );
    let expected: String = (1..=4)
        .map(|n| {
            format!(
                "crash_{n}: Silico built it; FreeCAD failed: \
                 FreeCAD crashed building it (signal: 9 (SIGKILL))\n"
            )
        })
        .chain(["4 of 6 cases differ from FreeCAD\n".to_owned()])
        .collect();
    assert_eq!(ran.stdout, expected, "{}", ran.stderr);
    assert_eq!(ran.status.and_then(|s| s.code()), Some(1));
}

#[test]
fn freecad_oracle_crashing_on_every_case_ends() {
    // Each case is a FreeCAD restart that crashes at once: the run still ends, every case
    // FreeCAD's failure.
    let temp = scratch("freecad-crash-all");
    let dir = temp.path();
    let freecad = freecad_killed_on_sphere_13(dir);
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    let all: Vec<Case> = (0..5)
        .map(|n| case(&format!("crash_{n}"), Shape::Sphere { radius: 13.0 }))
        .collect();
    write_cases(&cases, &all);
    let ran = cli(
        dir,
        &["run", cases.to_str().unwrap()],
        &[("SILICO_FREECADCMD", &freecad)],
        Duration::from_secs(120),
    );
    assert!(
        ran.stdout.ends_with("5 of 5 cases differ from FreeCAD\n"),
        "{}{}",
        ran.stdout,
        ran.stderr
    );
}

#[test]
fn freecad_that_never_starts_the_cases_is_an_error_not_a_pass() {
    // A FreeCAD that dies before reading the cases (a broken install), one that answers
    // more cases than it was given, and one that answers garbage: each a run error (exit
    // 2) saying what went wrong, never a verdict.
    let temp = scratch("freecad-garbage");
    let dir = temp.path();
    let cases = dir.join("cases");
    std::fs::create_dir_all(&cases).unwrap();
    write_cases(&cases, &[case("box", block(1.0, 2.0, 3.0))]);
    let stand_ins = [
        (
            "dies",
            "#!/bin/sh\necho 'ImportError: no module named Part' >&2\nexit 1\n",
            "FreeCAD crashed before starting the cases (exit status: 1); its last output:\n\
             ImportError: no module named Part",
        ),
        (
            "too-many",
            "#!/bin/sh\necho SILICO_ORACLE_READY\necho 'SILICO_ORACLE_CASE {\"failed\": \"a\"}'\n\
             echo 'SILICO_ORACLE_CASE {\"failed\": \"b\"}'\n",
            "FreeCAD gave 2 outcomes for 1 cases",
        ),
        (
            "garbage",
            "#!/bin/sh\necho SILICO_ORACLE_READY\necho 'SILICO_ORACLE_CASE {\"built\": {\"volume\": 1}'\n",
            "FreeCAD's outcome can't be read",
        ),
        (
            "nan",
            "#!/bin/sh\necho SILICO_ORACLE_READY\necho 'SILICO_ORACLE_CASE {\"built\": {\"volume\": NaN, \
             \"area\": 1, \"bounds\": null, \"faces\": 1, \"edges\": 1, \"vertices\": 1, \"valid\": true}}'\n",
            "FreeCAD's outcome can't be read",
        ),
    ];
    for (name, body, says) in stand_ins {
        let freecad = dir.join(name);
        executable(&freecad, body);
        let ran = cli(
            dir,
            &["run", cases.to_str().unwrap()],
            &[("SILICO_FREECADCMD", &freecad)],
            WAIT,
        );
        assert_eq!(ran.status.and_then(|s| s.code()), Some(2), "{name}");
        assert!(ran.stderr.contains(says), "{name}: {}", ran.stderr);
        assert!(ran.stdout.is_empty(), "{name}: {}", ran.stdout);
    }
}

// ---- Hostile case files ----

/// `text` as the only case file in a fresh directory, loaded.
fn load_text(dir: &Path, text: &[u8]) -> Result<Vec<Case>, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("cases.json"), text).unwrap();
    load(dir)
}

#[test]
fn hostile_case_files_are_refused_naming_the_file() {
    let temp = scratch("hostile-files");
    let dir = temp.path();
    let one = r#"{"name": "a", "note": "", "shape": {"box": {"dx": 1, "dy": 2, "dz": 3}}}"#;
    let deep = format!(
        r#"[{{"name": "a", "note": "", "shape": {}{}{}}}]"#,
        r#"{"translate": {"by": [0, 0, 0], "shape": "#.repeat(200),
        r#"{"box": {"dx": 1, "dy": 2, "dz": 3}}"#,
        "}}".repeat(200)
    );
    let hostile: Vec<(&str, Vec<u8>)> = vec![
        ("empty", b"".to_vec()),
        ("an object, not a list", one.as_bytes().to_vec()),
        ("a byte-order mark", format!("\u{feff}[{one}]").into_bytes()),
        ("invalid UTF-8", b"[\xff\xfe]".to_vec()),
        ("NaN", format!("[{}]", one.replace("\"dx\": 1", "\"dx\": NaN")).into_bytes()),
        ("1e400", format!("[{}]", one.replace("\"dx\": 1", "\"dx\": 1e400")).into_bytes()),
        ("unknown field", format!("[{}]", one.replace("\"note\"", "\"colour\": 1, \"note\"")).into_bytes()),
        ("unknown shape", format!("[{}]", one.replace("box", "pyramid")).into_bytes()),
        ("two shapes in one", format!("[{}]", one.replace(
            r#"{"box": {"dx": 1, "dy": 2, "dz": 3}}"#,
            r#"{"box": {"dx": 1, "dy": 2, "dz": 3}, "sphere": {"radius": 1}}"#,
        )).into_bytes()),
        ("duplicate field", format!("[{}]", one.replace("\"dx\": 1", "\"dx\": 1, \"dx\": 2")).into_bytes()),
        ("negative edge", br#"[{"name": "a", "note": "", "shape": {"fillet": {"shape": {"box": {"dx": 1, "dy": 2, "dz": 3}}, "edges": [-1], "radius": 0.1}}}]"#.to_vec()),
        ("edge past u32", br#"[{"name": "a", "note": "", "shape": {"fillet": {"shape": {"box": {"dx": 1, "dy": 2, "dz": 3}}, "edges": [4294967296], "radius": 0.1}}}]"#.to_vec()),
        ("two names alike", format!("[{one}, {one}]").into_bytes()),
        ("nested 200 deep", deep.into_bytes()),
    ];
    for (what, text) in hostile {
        let err = load_text(&dir.join("x"), &text).expect_err(what);
        assert!(
            err.contains("cases.json") || err.contains("two cases"),
            "{what}: {err}"
        );
    }
    // CRLF line endings are JSON whitespace: read as written.
    let crlf = format!("[\r\n{one}\r\n]\r\n");
    assert_eq!(load_text(&dir.join("x"), crlf.as_bytes()).unwrap().len(), 1);
    // A directory named like a case file, and a file given for the directory.
    let named = dir.join("named");
    std::fs::create_dir_all(named.join("cases.json")).unwrap();
    assert!(load(&named).unwrap_err().contains("cases.json"));
    let file = dir.join("named-file.json");
    std::fs::write(&file, format!("[{one}]")).unwrap();
    assert!(load(&file).is_err());
    // Two files with a case of one name.
    let twice = dir.join("twice");
    std::fs::create_dir_all(&twice).unwrap();
    std::fs::write(twice.join("a.json"), format!("[{one}]")).unwrap();
    std::fs::write(twice.join("b.json"), format!("[{one}]")).unwrap();
    assert!(load(&twice).unwrap_err().contains("two cases are named a"));
}

#[test]
fn a_case_file_cut_at_every_byte_is_refused_not_a_panic() {
    let full = std::fs::read(Path::new(CASES).join("primitives.json")).unwrap();
    let whole = load_text(&scratch("cut").path().join("x"), &full).unwrap();
    let temp = scratch("cut-each");
    let each = temp.path().join("x");
    for at in 0..full.len() {
        match load_text(&each, &full[..at]) {
            Err(e) => assert!(e.contains("cases.json"), "{at}: {e}"),
            // Only what's cut is trailing whitespace.
            Ok(cases) => {
                assert!(full[at..].iter().all(u8::is_ascii_whitespace), "{at}");
                assert_eq!(cases, whole);
            }
        }
    }
}

// ---- Determinism, and the comparison itself ----

#[test]
fn oracle_outcomes_dont_depend_on_case_order() {
    // Every hand case run forward and backward: each side's outcome for each case is the
    // same, bit for bit, wherever it comes in the run.
    let cases = load(Path::new(CASES)).unwrap();
    let mut reversed = cases.clone();
    reversed.reverse();
    let by_name = |cases: &[Case]| -> BTreeMap<String, (Outcome, Outcome)> {
        cases
            .iter()
            .map(|c| c.name.clone())
            .zip(outcomes(cases).unwrap())
            .collect()
    };
    let (forward, backward) = (by_name(&cases), by_name(&reversed));
    for (name, outcome) in &forward {
        assert_eq!(Some(outcome), backward.get(name), "{name}");
    }
}

/// SplitMix64, for edge-biased values.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value from a pool weighted to the edges: zeros, tolerance-sized steps, huge, NaN.
    fn value(&mut self, near: f64) -> f64 {
        let pool = [
            near,
            -near,
            0.0,
            -0.0,
            near * (1.0 + 1e-7),
            near * (1.0 - 1e-7),
            near * (1.0 + 1.1e-7),
            near + 1e-6,
            f64::NAN,
            f64::INFINITY,
            f64::MIN_POSITIVE,
            1e300,
            f64::from_bits(near.to_bits() + 1),
        ];
        pool[(self.next() % pool.len() as u64) as usize]
    }

    fn measured(&mut self) -> Measured {
        let low = [self.value(0.0), self.value(-5.0), self.value(1.0)];
        let high = [self.value(10.0), self.value(20.0), self.value(30.0)];
        Measured {
            volume: self.value(6000.0),
            area: self.value(2200.0),
            bounds: (!self.next().is_multiple_of(5)).then_some([low, high]),
            center: (!self.next().is_multiple_of(5))
                .then(|| [self.value(5.0), self.value(10.0), self.value(15.0)]),
            faces: (self.next() % 3) as u32 + 5,
            edges: 12,
            vertices: 8,
            valid: !self.next().is_multiple_of(4),
        }
    }
}

#[test]
fn the_comparison_is_symmetric_and_never_matches_nan() {
    // 5000 edge-biased pairs: swapping the sides never changes whether they differ; a NaN
    // anywhere is always a difference; and a measure always matches itself when it's
    // finite.
    let mut random = Random(20_261_010);
    for n in 0..5000 {
        let (a, b) = (random.measured(), random.measured());
        let (ours, theirs) = (Outcome::Built(a.clone()), Outcome::Built(b.clone()));
        let tolerance = [
            Tolerance::PLANAR,
            Tolerance::CURVED,
            Tolerance::CURVED_BOOLEAN,
        ][n % 3];
        let differences = |a: &Outcome, b: &Outcome| differences(a, b, tolerance);
        assert_eq!(
            differences(&ours, &theirs).is_empty(),
            differences(&theirs, &ours).is_empty(),
            "{n}: {a:?} {b:?}"
        );
        let values = |m: &Measured| {
            let mut v = vec![m.volume, m.area];
            if let Some([low, high]) = m.bounds {
                v.extend(low);
                v.extend(high);
            }
            if let Some(center) = m.center {
                v.extend(center);
            }
            v
        };
        if values(&a).iter().any(|v| v.is_nan()) {
            assert!(!differences(&ours, &ours).is_empty(), "{n}: {a:?}");
        }
        if values(&a).iter().all(|v| v.is_finite()) {
            assert!(differences(&ours, &ours).is_empty(), "{n}: {a:?}");
        }
    }
}

#[test]
fn a_chamfer_of_a_moved_shape_matches_freecad_where_it_was_moved() {
    // FreeCAD's PartDesign chamfer, which the oracle builds through, drops the placement of
    // the shape it's given: the chamfer of a moved box or cone came back at the origin, so
    // it couldn't match Silico's, and a Silico that lost the move would have.
    let chamfer = |shape, edges: Vec<u32>, size, flip| Shape::Chamfer {
        shape: Box::new(moved(shape, [3.0, -4.0, 5.0])),
        edges,
        size,
        flip,
    };
    let cases = [
        case(
            "moved_box",
            chamfer(
                block(10.0, 20.0, 30.0),
                vec![1, 6],
                Chamfer::TwoDistances {
                    distance: 1.0,
                    other: 2.0,
                },
                true,
            ),
        ),
        case(
            "moved_cone",
            chamfer(
                Shape::Cone {
                    radius1: 4.0,
                    radius2: 2.0,
                    height: 6.0,
                },
                vec![1],
                Chamfer::Equal { distance: 0.5 },
                true,
            ),
        ),
    ];
    let found = run(&cases).unwrap();
    assert!(found.refused.is_empty(), "{:?}", found.refused);
    assert!(
        found.differences.is_empty(),
        "{}",
        found
            .differences
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
