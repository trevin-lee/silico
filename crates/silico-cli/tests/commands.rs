//! M2.9: `silico check`, `fmt`, and `show` as a user or a script runs them, on the real
//! kernel worker (built beside `silico` when the workspace is built, as `pixi run test`
//! builds it). Expected values are written by hand: the example's measures (a 40 × 20 × 10
//! block: volume 8000), exit codes, and output shapes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[path = "common/run.rs"]
mod run;

use std::process::{Command, Output, Stdio};

const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
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
            .join(format!("cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, text: &str) -> PathBuf {
        let file = self.0.join("part.silico");
        std::fs::write(&file, text).unwrap();
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `silico ARGS` with the kernel and sketch workers beside it and the cache's secret in `scratch`.
fn silico(scratch: &Scratch, args: &[&str], file: &Path) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let sketch_worker = exe.with_file_name("silico-sketch-worker");
    assert!(
        sketch_worker.exists(),
        "{} isn't built: build the workspace (pixi run test does)",
        sketch_worker.display()
    );
    silico_with(scratch, args, file, &sketch_worker)
}

/// `silico ARGS` as [`silico`] runs it, its sketch worker `sketch_worker`.
fn silico_with(scratch: &Scratch, args: &[&str], file: &Path, sketch_worker: &Path) -> Output {
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
        .env("SILICO_SKETCH_WORKER", sketch_worker)
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    run::output(cmd)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn fmt_writes_fingerprints_and_check_then_finds_nothing_wrong() {
    let scratch = Scratch::new("fmt-check");
    let file = scratch.file(EXAMPLE);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("wrote the fingerprint of k7f2qx3m"),
        "{}",
        stderr(&out)
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains(r#""fingerprint": {"volume": 8000,"#),
        "{text}"
    );
    // The file keeps its permissions, and nothing is left beside it but the cache.
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
    let mut left: Vec<String> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".silico-cache", "cache-home", "part.silico"]);
    // Formatted, it's clean, and formatting again changes nothing.
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
    let out = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn check_json_is_a_line_an_agent_reads() {
    let scratch = Scratch::new("json");
    let file = scratch.file(EXAMPLE);
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["ok"], true);
    assert_eq!(line["problems"], serde_json::json!([]));
    assert_eq!(line["total"], 0);
    assert_eq!(line["file"], file.display().to_string());
}

#[test]
fn drift_is_exit_1_naming_the_feature() {
    let scratch = Scratch::new("drift");
    let file = scratch.file(EXAMPLE);
    assert_eq!(silico(&scratch, &["fmt"], &file).status.code(), Some(0));
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1),
    )
    .unwrap();
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(": drift: "), "{}", stderr(&out));
    assert!(stderr(&out).contains("k7f2qx3m"), "{}", stderr(&out));
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(1));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["ok"], false);
    assert_eq!(line["problems"][0]["code"], "drift");
    assert_eq!(line["problems"][0]["feature"], "k7f2qx3m");
    assert_eq!(line["total"], 1);
}

#[test]
fn fmt_check_writes_nothing_and_says_the_file_would_change() {
    let scratch = Scratch::new("fmt-dry");
    let file = scratch.file(EXAMPLE);
    let out = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("not in canonical form"),
        "{}",
        stderr(&out)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), EXAMPLE);
}

#[test]
fn a_document_that_doesnt_pass_the_gate_is_exit_1_and_left_as_it_was() {
    let scratch = Scratch::new("invalid");
    let text = EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#);
    let file = scratch.file(&text);
    for command in ["check", "fmt"] {
        let out = silico(&scratch, &[command], &file);
        assert_eq!(out.status.code(), Some(1), "{command}: {}", stderr(&out));
        assert!(stderr(&out).contains("invalid_input"), "{}", stderr(&out));
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn show_prints_the_outline_without_a_kernel() {
    let scratch = Scratch::new("show");
    let file = scratch.file(EXAMPLE);
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .arg("show")
        .arg(&file)
        .env("SILICO_KERNEL_WORKER", "/nonexistent")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "k7f2qx3m \"Block\" box length 40, width 20, height 10, at (0, 0, 0)\nbodies k7f2qx3m\n"
    );
}

#[test]
fn a_file_that_cant_be_read_or_a_bad_command_is_exit_2() {
    let scratch = Scratch::new("missing");
    let missing = scratch.0.join("nothing.silico");
    for args in [&["check"][..], &["fmt"], &["show"]] {
        let out = silico(&scratch, args, &missing);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr(&out).contains("couldn't be read"),
            "{}",
            stderr(&out)
        );
    }
    let file = scratch.file(EXAMPLE);
    // `show` and `textconv` take one file.
    for command in ["show", "textconv"] {
        let out = Command::new(env!("CARGO_BIN_EXE_silico"))
            .args([command])
            .arg(&file)
            .arg(&file)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{command}");
        assert!(stderr(&out).contains("usage: silico"), "{}", stderr(&out));
        assert!(out.stdout.is_empty(), "{command}");
    }
    for args in [&["chekc"][..], &["check", "--jsno"], &["fmt", "--json"]] {
        let out = silico(&scratch, args, &file);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).contains("usage: silico"), "{}", stderr(&out));
    }
}

/// The example's block and a fillet of it too large to build.
const FAILING: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 50}
  ]
}
"#;

#[test]
fn a_feature_that_fails_to_build_is_exit_1_for_check_and_fmt_which_still_writes() {
    // Found by the milestone's tests review: no test had a feature fail to build, so `fmt`
    // exiting 0 for one passed (docs/cli.md: the file is still written, exit 1).
    let scratch = Scratch::new("failing");
    let file = scratch.file(FAILING);
    let check = silico(&scratch, &["check"], &file);
    assert_eq!(check.status.code(), Some(1), "{}", stderr(&check));
    let fmt = silico(&scratch, &["fmt"], &file);
    assert_eq!(fmt.status.code(), Some(1), "{}", stderr(&fmt));
    assert!(stderr(&fmt).contains("f0000001"), "{}", stderr(&fmt));
    // Written: the block, which built, has its fingerprint (8000, the block's volume).
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains(r#""volume": 8000"#), "{written}");
}

#[test]
fn the_worst_of_several_files_decides_the_exit_whatever_their_order() {
    // Found by the milestone's tests review: the orders tested were all ones a fold that
    // keeps the last file's outcome passes too. A file that can't be read (2), then one
    // that doesn't pass the gate (1): 2.
    let scratch = Scratch::new("worst");
    let bad = scratch.file("{");
    let missing = scratch.0.join("missing.silico");
    let missing = missing.to_str().unwrap();
    for command in ["check", "fmt"] {
        let out = silico(&scratch, &[command, missing], &bad);
        assert_eq!(out.status.code(), Some(2), "{command}: {}", stderr(&out));
    }
}

/// `git ARGS` in `dir`, with no user or system configuration.
fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
    out
}

/// A git repository in `scratch` with `silico textconv` as `*.silico`'s diff driver, as
/// docs/cli.md sets it up, and `text` committed as `part.silico`.
fn repository(scratch: &Scratch, text: &str) -> PathBuf {
    let dir = scratch.0.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join(".gitattributes"), "*.silico diff=silico\n").unwrap();
    let driver = format!("{} textconv", env!("CARGO_BIN_EXE_silico"));
    git(&dir, &["config", "diff.silico.textconv", &driver]);
    std::fs::write(dir.join("part.silico"), text).unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "part"]);
    dir
}

/// The lines `git diff` says were taken out and put in.
fn changed(dir: &Path) -> (Vec<String>, Vec<String>) {
    let out = git(dir, &["diff", "--no-color", "-U0"]);
    let text = String::from_utf8(out.stdout).unwrap();
    let lines = |sign: char| -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with(sign) && !l.starts_with("---") && !l.starts_with("+++"))
            .map(|l| l[1..].to_owned())
            .collect()
    };
    (lines('-'), lines('+'))
}

#[test]
fn git_diff_through_textconv_shows_a_one_value_edit_as_one_line() {
    let scratch = Scratch::new("textconv");
    let dir = repository(&scratch, EXAMPLE);
    std::fs::write(
        dir.join("part.silico"),
        EXAMPLE.replace(r#""length": 40"#, r#""length": 45"#),
    )
    .unwrap();
    assert_eq!(
        changed(&dir),
        (
            vec![r#"k7f2qx3m "Block" box length 40, width 20, height 10, at (0, 0, 0)"#.to_owned()],
            vec![r#"k7f2qx3m "Block" box length 45, width 20, height 10, at (0, 0, 0)"#.to_owned()],
        )
    );
}

#[test]
fn git_diff_through_textconv_shows_a_file_that_isnt_a_document_as_it_is() {
    // A merge's conflict markers: not a document, so shown as written, not refused.
    let scratch = Scratch::new("textconv-raw");
    let dir = repository(&scratch, EXAMPLE);
    let conflicted = EXAMPLE.replace(
        r#"    {"id": "k7f2qx3m""#,
        "<<<<<<< ours\n    {\"id\": \"k7f2qx3m\"",
    );
    std::fs::write(dir.join("part.silico"), &conflicted).unwrap();
    let (out, put_in) = changed(&dir);
    assert_eq!(
        out,
        [
            r#"k7f2qx3m "Block" box length 40, width 20, height 10, at (0, 0, 0)"#,
            "bodies k7f2qx3m",
        ]
    );
    let expected: Vec<String> = conflicted.lines().map(str::to_owned).collect();
    assert_eq!(put_in, expected);
}

#[test]
fn textconv_prints_a_file_too_long_to_be_a_document_whole() {
    let scratch = Scratch::new("textconv-long");
    let file = scratch.0.join("big.silico");
    let len = silico_core::document::json::MAX_INPUT_LEN + 4096;
    let text: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).collect();
    std::fs::write(&file, &text).unwrap();
    let out = silico(&scratch, &["textconv"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stdout == text, "{} bytes of {len}", out.stdout.len());
    let missing = scratch.0.join("nothing.silico");
    let out = silico(&scratch, &["textconv"], &missing);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("couldn't be read"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn fmt_of_a_drifted_file_rewrites_it_and_is_clean() {
    // The drift it fixes isn't a problem it reports: exit 0, and the file checks clean.
    let scratch = Scratch::new("fmt-drift");
    let file = scratch.file(EXAMPLE);
    assert_eq!(silico(&scratch, &["fmt"], &file).status.code(), Some(0));
    let saved = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        saved.replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1),
    )
    .unwrap();
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains(": drift:"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("rewrote the fingerprint of k7f2qx3m"),
        "{}",
        stderr(&out)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);
}

#[test]
fn checking_several_files_starts_its_workers_once_whatever_their_directories() {
    // Found by the review: each file started a pool of its own, files × cores workers
    // with nothing to build. Counted from the workers' start log.
    let scratch = Scratch::new("one-pool");
    let started = |files: &[&str]| -> usize {
        let paths: Vec<PathBuf> = files.iter().map(|f| scratch.0.join(f)).collect();
        kernel_workers_started(&scratch, &paths)
    };
    std::fs::create_dir(scratch.0.join("x")).unwrap();
    std::fs::create_dir(scratch.0.join("y")).unwrap();
    // Each file a block of its own length, so none is in the cache before its run, and its
    // rebuild (requests one after another) needs a worker: the one pool's one worker, for
    // every file of a run.
    let names = [
        "x/a.silico",
        "x/b.silico",
        "x/c.silico",
        "y/d.silico",
        "x/e.silico",
        "y/f.silico",
        "x/g.silico",
    ];
    for (i, name) in names.iter().enumerate() {
        let block = EXAMPLE.replace(r#""length": 40"#, &format!(r#""length": {}"#, 40 + i));
        std::fs::write(scratch.0.join(name), block).unwrap();
    }
    assert_eq!(started(&["x/a.silico"]), 1);
    assert_eq!(started(&["x/b.silico", "x/c.silico"]), 1);
    // Directories in turn, each with its own cache, on the one pool (the review found a
    // pool started at every change of directory).
    assert_eq!(
        started(&["y/d.silico", "x/e.silico", "y/f.silico", "x/g.silico"]),
        1
    );
    for dir in ["x", "y"] {
        assert!(scratch.0.join(dir).join(".silico-cache").is_dir(), "{dir}");
    }
}

/// What `tests/contract/check-line.json` pins: the line `silico check --json` writes for
/// a document with problems, and for a file it couldn't check.
fn check_lines(scratch: &Scratch) -> serde_json::Value {
    let bad = scratch.0.join("bad.silico");
    std::fs::write(&bad, EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#)).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_silico"));
    cmd.args(["check", "--json", "bad.silico", "missing.silico"])
        .current_dir(&scratch.0)
        .env("SILICO_KERNEL_WORKER", "/nonexistent")
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    let out = run::output(cmd);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let [problems, couldnt] = &lines[..] else {
        panic!("{lines:?}");
    };
    serde_json::json!({"problems": problems, "couldnt_check": couldnt})
}

#[test]
fn check_jsons_lines_are_the_contracts() {
    // The JSON agents read (protected, tests/contract/): a document's problems, each an
    // error in the core's contract shape, and a file that couldn't be checked, its error in
    // that shape too (`io_failed`, Trevin's, 2026-10-16).
    let scratch = Scratch::new("contract");
    let path = format!(
        "{}/tests/contract/check-line.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let contract: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(check_lines(&scratch), contract);
}

/// M3.4: a 40 × 20 rectangle, fully constrained, its first corner at the origin, drawn
/// where it solves.
const RECTANGLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy",
     "points": [
       {"id": "p0000001", "x": 0, "y": 0, "construction": false},
       {"id": "p0000002", "x": 40, "y": 0, "construction": false},
       {"id": "p0000003", "x": 40, "y": 20, "construction": false},
       {"id": "p0000004", "x": 0, "y": 20, "construction": false}
     ],
     "lines": [
       {"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false},
       {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false},
       {"id": "n0000003", "from": "p0000003", "to": "p0000004", "construction": false},
       {"id": "n0000004", "from": "p0000004", "to": "p0000001", "construction": false}
     ],
     "circles": [], "arcs": [],
     "constraints": [
       {"id": "k0000001", "type": "horizontal", "line": "n0000001"},
       {"id": "k0000002", "type": "horizontal", "line": "n0000003"},
       {"id": "k0000003", "type": "vertical", "line": "n0000002"},
       {"id": "k0000004", "type": "vertical", "line": "n0000004"},
       {"id": "k0000005", "type": "length", "line": "n0000001", "value": 40, "driving": true},
       {"id": "k0000006", "type": "length", "line": "n0000002", "value": 20, "driving": true},
       {"id": "k0000007", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true},
       {"id": "k0000008", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}
     ]}
  ]
}
"#;

#[test]
fn a_sketch_edited_by_hand_is_drift_until_fmt_writes_its_solve_and_then_nothing_changes() {
    // The roadmap's M3.4 case: a dimension edited by hand (the rectangle's 40 made 50), so
    // the corners the file stores no longer solve it. `check` names the sketch and the two
    // corners that move (x 40 to 50: p2 and p3; p1 is held at the origin, p4 on x = 0);
    // `fmt` writes the solve and says so; after it, `check` is clean and `fmt` again
    // changes no byte.
    let scratch = Scratch::new("sketch-drift");
    let file = scratch.file(RECTANGLE);
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(silico(&scratch, &["check"], &file).status.code(), Some(0));
    let saved = std::fs::read_to_string(&file).unwrap();
    let edited = saved.replacen(r#""value": 40,"#, r#""value": 50,"#, 1);
    assert_ne!(edited, saved);
    std::fs::write(&file, &edited).unwrap();

    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["problems"][0]["code"], "drift", "{line}");
    assert_eq!(line["problems"][0]["feature"], "s0000001");
    assert_eq!(
        line["problems"][0]["elements"],
        serde_json::json!(["p0000002", "p0000003"])
    );
    assert_eq!(line["total"], 1);

    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("wrote the solve of sketch s0000001"),
        "{}",
        stderr(&out)
    );
    let solved = std::fs::read_to_string(&file).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&solved).unwrap();
    let points = &doc["features"][0]["points"];
    for (i, want) in [
        (0, [0.0, 0.0]),
        (1, [50.0, 0.0]),
        (2, [50.0, 20.0]),
        (3, [0.0, 20.0]),
    ] {
        let got = [
            points[i]["x"].as_f64().unwrap(),
            points[i]["y"].as_f64().unwrap(),
        ];
        assert!(
            (got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9,
            "point {i}: {got:?}, want {want:?}"
        );
    }
    assert_eq!(silico(&scratch, &["check"], &file).status.code(), Some(0));
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("wrote"), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), solved);
}

#[test]
fn a_sketch_worker_that_cant_be_found_is_exit_2_and_nothing_is_written() {
    // Round 2's architecture and quality lenses: SILICO_SKETCH_WORKER naming no file blamed
    // the sketch (exit 1, and fmt wrote the file); it's the solver's failure, as the
    // kernel's is: exit 2, and the file as it was.
    let scratch = Scratch::new("no-sketch-worker");
    let file = scratch.file(RECTANGLE);
    let run = |args: &[&str]| silico_with(&scratch, args, &file, Path::new("/nowhere"));
    let out = run(&["check"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("worker_crashed"), "{}", stderr(&out));
    let out = run(&["fmt"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), RECTANGLE);
}

/// How many kernel workers `silico check FILES` started (from the workers' start log),
/// failing unless it found nothing wrong.
fn kernel_workers_started(scratch: &Scratch, files: &[PathBuf]) -> usize {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.arg("check")
        .args(files)
        .env(
            "SILICO_KERNEL_WORKER",
            exe.with_file_name("silico-kernel-worker"),
        )
        .env(
            "SILICO_SKETCH_WORKER",
            exe.with_file_name("silico-sketch-worker"),
        )
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env("SILICO_LOG", "silico_core::worker=debug")
        .stdin(Stdio::null());
    let out = run::output(cmd);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    stderr(&out).matches("started kernel worker").count()
}

#[test]
fn check_starts_only_the_kernel_workers_its_rebuild_needs() {
    // A slot for each core, but a worker only for the requests run at once: a block's
    // rebuild, its requests one after another, starts one; from the cache, none. (A worker
    // a core cost each run its start, which on a loaded Linux machine was most of the time
    // of a small document's check.)
    let scratch = Scratch::new("workers-needed");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, EXAMPLE).unwrap();
    assert_eq!(
        kernel_workers_started(&scratch, std::slice::from_ref(&file)),
        1
    );
    assert_eq!(
        kernel_workers_started(&scratch, &[file]),
        0,
        "rebuilt from the cache"
    );
}
