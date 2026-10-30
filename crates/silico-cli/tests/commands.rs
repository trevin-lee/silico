//! M2.9: `silico check`, `fmt`, and `show` as a user or a script runs them, on the real
//! kernel worker (built beside `silico` when the workspace is built, as `pixi run test`
//! builds it). Expected values are written by hand: the example's measures (a 40 × 20 × 10
//! block: volume 8000), exit codes, and output shapes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[path = "common/canonical.rs"]
mod canonical;
#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/run.rs"]
mod run;
#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

use std::process::{Command, Output, Stdio};

const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// The entries of the cache `dir`, in order of path, found as the cache finds its own: a
/// regular file named by 62 lowercase hex digits in a subdirectory named by two, no link
/// followed. A copy of silico-core's `tests/common/cache_control.rs` `entries` (this crate's
/// tests can't load that file): keep the two alike.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let hex = |name: &str, digits: usize| {
        name.len() == digits && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    let mut found = Vec::new();
    for sub in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let named = sub.file_name().to_str().is_some_and(|name| hex(name, 2));
        if !named || !std::fs::symlink_metadata(sub.path()).unwrap().is_dir() {
            continue;
        }
        for file in std::fs::read_dir(sub.path()).unwrap().flatten() {
            let named = file.file_name().to_str().is_some_and(|name| hex(name, 62));
            if named && std::fs::symlink_metadata(file.path()).unwrap().is_file() {
                found.push(file.path());
            }
        }
    }
    found.sort();
    found
}

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
fn check_says_warnings_on_stderr_as_warnings_and_they_never_change_the_exit_code() {
    // The example, saved by hand with no fingerprint: one warning, exit 0.
    let scratch = Scratch::new("warnings");
    let file = scratch.file(EXAMPLE);
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let said = format!(
        "{}: warning: fingerprint_missing: k7f2qx3m built, but its file stores no \
         fingerprint, so a change in what it builds isn't caught as drift (feature k7f2qx3m, \
         parameter fingerprint). Suggestion: format the document to store it.\n",
        file.display()
    );
    assert_eq!(stderr(&out), said);
    // `--json`'s line is its contract's (check-line.json), with no warnings in it: they're
    // said on stderr, as without it.
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), said);
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        line,
        serde_json::json!({"file": file.display().to_string(), "ok": true, "problems": [], "total": 0})
    );
    // Formatted, its fingerprint stored: nothing to say.
    assert_eq!(silico(&scratch, &["fmt"], &file).status.code(), Some(0));
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
    // Its volume edited by hand (drift, a problem), and a second box added with no
    // fingerprint (a warning): exit 1, the problem said first, then the warning.
    let text = std::fs::read_to_string(&file).unwrap();
    let edited = text
        .replacen(r#""volume": 8000,"#, r#""volume": 8001,"#, 1)
        .replacen(
            "}\n  ]",
            "},\n    {\"id\": \"m0000002\", \"type\": \"box\", \"v\": 1, \"name\": \"Other\", \"position\": [50, 0, 0], \"length\": 1, \"width\": 1, \"height\": 1}\n  ]",
            1,
        );
    assert_eq!(edited.matches("m0000002").count(), 1, "{text}");
    std::fs::write(&file, &edited).unwrap();
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let lines: Vec<String> = stderr(&out).lines().map(str::to_owned).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with(&format!("{}: drift: ", file.display())),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with(&format!(
            "{}: warning: fingerprint_missing: m0000002 built",
            file.display()
        )),
        "{lines:?}"
    );
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
fn a_document_of_another_naming_scheme_is_exit_1_naming_the_field_and_left_as_it_was() {
    // M4.0: names are never read under a scheme other than their own.
    let scratch = Scratch::new("other-scheme");
    let text = EXAMPLE.replacen("\n  \"features\"", "\n  \"naming\": 4,\n  \"features\"", 1);
    assert!(text.contains("\"naming\": 4,"));
    let file = scratch.file(&text);
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["ok"], false);
    assert_eq!(line["total"], 1);
    let problem = &line["problems"][0];
    assert_eq!(problem["code"], "invalid_input");
    assert_eq!(problem["parameter"], "naming");
    assert_eq!(
        problem["message"],
        "the document's names were written under naming scheme 4, which this Silico doesn't \
         read, at line 4, column 13"
    );
    assert_eq!(
        problem["suggestion"],
        "this Silico reads naming scheme 5: open it in the Silico that wrote it"
    );
    for command in ["fmt", "check"] {
        let out = silico(&scratch, &[command], &file);
        assert_eq!(out.status.code(), Some(1), "{command}: {}", stderr(&out));
        assert!(stderr(&out).contains("naming scheme 4"), "{}", stderr(&out));
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
fn fmt_check_of_a_feature_that_fails_to_build_writes_nothing_and_is_exit_1() {
    // Found by M2.9's tests review: no test ran `fmt --check` on a document with a failed
    // feature. docs/cli.md: --check writes nothing, and a document with problems is exit 1,
    // whether or not it's in canonical form.
    let scratch = Scratch::new("failing-check");
    let file = scratch.file(FAILING);
    // Not in canonical form (the block has no fingerprint yet): exit 1, saying both.
    let out = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("f0000001"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not in canonical form"),
        "{}",
        stderr(&out)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), FAILING);
    // In canonical form, as fmt writes it: still exit 1, for the feature alone.
    let fmt = silico(&scratch, &["fmt"], &file);
    assert_eq!(fmt.status.code(), Some(1), "{}", stderr(&fmt));
    let formatted = std::fs::read_to_string(&file).unwrap();
    let out = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("f0000001"), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("not in canonical form"),
        "{}",
        stderr(&out)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), formatted);
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

/// What `silico check --json` writes for a document with problems, and for a file it
/// couldn't check: its two lines, as text.
fn check_text(scratch: &Scratch) -> String {
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
    String::from_utf8(out.stdout).unwrap()
}

/// What `tests/contract/check-line.json` pins: the line `silico check --json` writes for
/// a document with problems, and for a file it couldn't check.
fn check_lines(scratch: &Scratch) -> serde_json::Value {
    let lines: Vec<serde_json::Value> = check_text(scratch)
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

#[test]
fn check_jsons_lines_hold_their_keys_in_the_contracts_order_in_any_build() {
    // The lines' text, not a value read from it (which compares objects whatever their
    // keys' order): every object's keys sorted, as tests/contract/check-line.json holds
    // them, whether or not something in the build turns on serde_json's `preserve_order`
    // (Rerun does, in the workspace's build), which a line built as a `serde_json::Value`
    // came out in the order its keys were added in, the error's in its fields' order.
    let scratch = Scratch::new("contract-text");
    assert_eq!(
        check_text(&scratch),
        concat!(
            r#"{"file":"bad.silico","ok":false,"problems":[{"candidates":[],"cause":null,"#,
            r#""code":"invalid_input","elements":[],"elements_total":0,"feature":"k7f2qx3m","#,
            r#""message":"box length must be a length above 1e-7 mm and at most 1e6 mm (a "#,
            r#"kilometre), got 0","parameter":"length","suggestion":"give the box a valid "#,
            r#"length"}],"total":1}"#,
            "\n",
            r#"{"error":{"candidates":[],"cause":null,"code":"io_failed","elements":[],"#,
            r#""elements_total":0,"feature":null,"message":"couldn't be read: No such file "#,
            r#"or directory (os error 2)","parameter":null,"suggestion":"check the file's "#,
            r#"name, and that it's a regular file this user may read"},"file":"missing.silico","#,
            r#""ok":false}"#,
            "\n",
        )
    );
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
fn a_sketch_worker_that_cant_be_found_is_exit_2_and_its_sketch_kept_as_stored() {
    // Round 2's architecture and quality lenses: SILICO_SKETCH_WORKER naming no file blamed
    // the sketch (exit 1, and fmt wrote the file); it's the solver's failure, as the
    // kernel's is: exit 2 (worker_unavailable: it can't be had). fmt saves around it, as
    // the `format` command does (decision 32): the sketch the fault stopped keeps what its
    // file holds, so the file is written only into canonical form, and no solve is said.
    let scratch = Scratch::new("no-sketch-worker");
    let file = scratch.file(RECTANGLE);
    let run = |args: &[&str]| silico_with(&scratch, args, &file, Path::new("/nowhere"));
    let out = run(&["check"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("worker_unavailable"),
        "{}",
        stderr(&out)
    );
    let out = run(&["fmt"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("worker_unavailable"),
        "{}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("wrote"), "{}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        canonical::canonical(RECTANGLE)
    );
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

#[test]
fn an_open_sketch_swept_twice_names_each_failed_pad_once() {
    // The architecture review's case (M3, round 1), in Fable's shape: a square's outline
    // with its fourth side left out, padded twice. Its two open ends, p0000001 (of
    // n0000001) and p0000004 (of n0000003), are the profile's two refusals; each pad fails
    // with one whole problem naming it, the sketch's four elements at fault its own, the
    // refusals and their advice its cause. Two problems, and none names the sketch.
    let scratch = Scratch::new("open-twice");
    let points = [
        fixture::point("p0000001", 0.0, 0.0),
        fixture::point("p0000002", 10.0, 0.0),
        fixture::point("p0000003", 10.0, 10.0),
        fixture::point("p0000004", 0.0, 10.0),
    ];
    let lines = [
        fixture::line("n0000001", "p0000001", "p0000002"),
        fixture::line("n0000002", "p0000002", "p0000003"),
        fixture::line("n0000003", "p0000003", "p0000004"),
    ];
    let sketch = sweeps::sketch_of("s0000001", "xy", &points, &lines, &[], &[]);
    let extent = sweeps::one_side("1");
    let file = scratch.file(&sweeps::document(&[
        &sketch,
        &sweeps::pad("d0000001", None, "s0000001", &extent),
        &sweeps::pad("d0000009", None, "s0000001", &extent),
    ]));
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["total"], 2, "{line}");
    let advice = "end another curve at the point to close the outline, or make the curve construction geometry";
    let cause = format!(
        "point p0000001 ends line n0000001 and no other profile curve, so the profile is open there ({advice}); point p0000004 ends line n0000003 and no other profile curve, so the profile is open there"
    );
    for (i, pad) in ["d0000001", "d0000009"].into_iter().enumerate() {
        let problem = &line["problems"][i];
        assert_eq!(problem["code"], "invalid_input", "{line}");
        assert_eq!(problem["feature"], pad, "{line}");
        assert_eq!(problem["parameter"], "sketch", "{line}");
        assert_eq!(
            problem["elements"],
            serde_json::json!(["n0000001", "n0000003", "p0000001", "p0000004"]),
            "{line}"
        );
        assert_eq!(problem["elements_total"], 4, "{line}");
        assert_eq!(
            problem["message"],
            format!("pad {pad} sweeps sketch s0000001, which makes no profile (2 refusals)")
        );
        assert_eq!(problem["cause"], cause, "{line}");
        assert_eq!(
            problem["suggestion"],
            "fix sketch s0000001 where the cause says, or sweep another sketch"
        );
    }
    let problems = line["problems"].as_array().unwrap();
    assert!(
        problems.iter().all(|p| p["feature"] != "s0000001"),
        "{line}"
    );
    // `fmt` lists the same two, and nothing against the sketch.
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let said = stderr(&out);
    for pad in ["d0000001", "d0000009"] {
        assert!(
            said.contains(&format!(
                "pad {pad} sweeps sketch s0000001, which makes no profile (2 refusals) (feature {pad}, elements n0000001, n0000003, p0000001, p0000004, parameter sketch)"
            )),
            "{said}"
        );
    }
    assert!(!said.contains("feature s0000001"), "{said}");
}

#[test]
fn a_groove_whose_other_region_is_off_the_axis_is_refused_for_its_narrow_slit() {
    // The robustness review's case (M3, round 3): a groove of two rectangles, x 0..10
    // touching the y axis and x −2..−1 off it, into a box, 600 nanodegrees short of a whole
    // turn. The slit is narrowest at the second's near side, 1 mm off the axis:
    // 1 · 600e-9 · π/180 = 1.0472e-8 mm, one place, so it's refused naming the groove's
    // angle (measured at 10 mm, 1.05e-7, it passed and OCCT cut itself).
    let scratch = Scratch::new("groove-slit");
    let file = scratch.file(&sweeps::document(&[
        &sweeps::box_at("b0000001", [-20.0, -1.0, -20.0], 40.0, 3.0, 40.0),
        &sweeps::rectangles(
            "s0000001",
            "xy",
            &[[0.0, 0.0, 10.0, 5.0], [-2.0, 0.0, -1.0, 5.0]],
        ),
        &sweeps::turned(
            "g0000001",
            "groove",
            Some("b0000001"),
            "s0000001",
            "y_axis",
            &sweeps::angle("359.9999994"),
        ),
    ]));
    let out = silico(&scratch, &["check", "--json"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let refused: Vec<&serde_json::Value> = line["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["feature"] == "g0000001")
        .collect();
    assert_eq!(refused.len(), 1, "{line}");
    assert_eq!(refused[0]["code"], "invalid_input", "{line}");
    assert_eq!(refused[0]["parameter"], "extent.angle", "{line}");
    let message = refused[0]["message"].as_str().unwrap();
    assert!(
        message.ends_with(
            " mm at its profile's point 1 mm from the axis: within 1e-7 mm, its ends would be one place"
        ),
        "{message}"
    );
}

#[test]
fn a_revolved_barrels_fingerprint_is_its_volume_by_hand_and_check_passes() {
    // A sketch on XZ of x = 1 to the arc through (4, −4), (5, 0), and (4, 4) (centre
    // (−3.5, 0), radius 8.5, its circle reaching past the axis), revolved a whole turn
    // about the sketch's y (model z): a surface of revolution OCCT 7.9.3's precise
    // measure had 2.2e-7 low (526.334197) until patch 0003 (docs/ports/measure.md). By
    // hand, π ∫ (xo² − 1) dz over z from −4 to 4, xo = −3.5 + √(8.5² − z²):
    // π (668 − 128/3 − 7 (30 + 8.5² asin(4/8.5))) = 526.33431398915, written to six
    // decimals as 526.334314; its area, the two annuli 15π each, the bore 16π, and the
    // side by Pappus, 4π · 8.5 (−3.5 asin(4/8.5) + 4): 388.5995488823284, 388.599549.
    let scratch = Scratch::new("barrel");
    let points = [
        fixture::point("p0000001", 1.0, -4.0),
        fixture::point("p0000002", 4.0, -4.0),
        fixture::point("p0000003", 4.0, 4.0),
        fixture::point("p0000004", 1.0, 4.0),
        fixture::point("p0000005", -3.5, 0.0),
    ];
    let lines = [
        fixture::line("n0000001", "p0000001", "p0000002"),
        fixture::line("n0000002", "p0000003", "p0000004"),
        fixture::line("n0000003", "p0000004", "p0000001"),
    ];
    let arcs = [fixture::arc("a0000001", "p0000005", "p0000002", "p0000003")];
    let sketch = sweeps::sketch_of("s0000001", "xz", &points, &lines, &[], &arcs);
    let revolve = sweeps::turned(
        "r0000001",
        "revolve",
        None,
        "s0000001",
        "y_axis",
        sweeps::TURN,
    );
    let file = scratch.file(&sweeps::document(&[&sketch, &revolve]));
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = std::fs::read_to_string(&file).unwrap();
    // Its tight box by hand, (−5, −5, −4) to (5, 5, 4): the arc's widest point, x = 5 at
    // z = 0, turned. Its arc crosses its circle's angle 0, whose face was boxed from x = −4
    // before each arc edge took OCCT's own range (docs/ports/measure.md).
    assert!(
        text.contains(
            r#""fingerprint": {"volume": 526.334314, "area": 388.599549, "bbox": [-5, -5, -4, 5, 5, 4],"#
        ),
        "{text}"
    );
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
}

/// `silico schema ARGS`, which needs no worker and no file.
fn schema(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_silico"))
        .arg("schema")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// The core's committed contract file `name`.
fn core_contract(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn schema_prints_the_documents_or_a_commands_as_committed() {
    // Without a name, the document's: exactly its committed snapshot.
    let out = schema(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        core_contract("document-schema.json")
    );
    assert_eq!(
        String::from_utf8(schema(&["document"]).stdout).unwrap(),
        core_contract("document-schema.json")
    );
    for (command, _) in silico_core::command::wire::COMMANDS {
        let out = schema(&[command]);
        assert_eq!(out.status.code(), Some(0), "{command}: {}", stderr(&out));
        assert!(out.stderr.is_empty(), "{command}");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            core_contract(&format!("commands/{command}.json")),
            "{command}"
        );
    }
    // A name that's neither: exit 2, the core's refusal, its candidates, and nothing on
    // stdout.
    let out = schema(&["envelope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(
        stderr(&out),
        "silico schema: invalid_input: of must be document or the name of a command with a \
         schema, got envelope (parameter of). Suggestion: give one of the candidates. \
         Candidates: document, version, open, close, status, cancel, begin, commit, rollback, \
         format, add_feature, set_field, remove_feature, replace_text, text, outline, check, \
         cache_clear, elements, query, schema, mass_properties, distance, angle, render, \
         read_logs, export, move_feature, timeline.\n"
    );
    // Two names: a usage error.
    let out = schema(&["query", "elements"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("silico schema [NAME]"),
        "{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty());
}

/// `silico ARGS FILE` with the sketch worker beside it and the kernel worker `kernel` (one
/// that can't be had, say), the cache's secret in `scratch`.
fn silico_on_kernel(scratch: &Scratch, args: &[&str], file: &Path, kernel: &Path) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .arg(file)
        .env("SILICO_KERNEL_WORKER", kernel)
        .env(
            "SILICO_SKETCH_WORKER",
            exe.with_file_name("silico-sketch-worker"),
        )
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    run::output(cmd)
}

#[test]
fn a_kernel_fault_is_said_beside_the_documents_own_problems_and_is_exit_2() {
    // M4.2's thin client (decision 32, the design's section 6): a kernel fault isn't the
    // document's, so it's given once, beside the problems the rebuild found without the
    // kernel, never instead of them. Here the kernel can't be had (no worker at the path
    // named); an open outline's pad fails on its own (the sketch worker solves it, and the
    // profile it makes is refused before any kernel request), and the block after it needs
    // the kernel. check: the pad's problem, then the fault, exit 2; --json: the fault as
    // the line's error (a file that couldn't be checked, the contract's second shape).
    let scratch = Scratch::new("fault-beside");
    let points = [
        fixture::point("p0000001", 0.0, 0.0),
        fixture::point("p0000002", 10.0, 0.0),
        fixture::point("p0000003", 10.0, 10.0),
    ];
    let lines = [
        fixture::line("n0000001", "p0000001", "p0000002"),
        fixture::line("n0000002", "p0000002", "p0000003"),
    ];
    let sketch = sweeps::sketch_of("s0000001", "xy", &points, &lines, &[], &[]);
    let block = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
    let text = sweeps::document(&[
        &sketch,
        &sweeps::pad("d0000001", None, "s0000001", &sweeps::one_side("1")),
        block,
    ]);
    let file = scratch.file(&text);
    let nowhere = scratch.0.join("no-kernel-worker");
    let out = silico_on_kernel(&scratch, &["check"], &file, &nowhere);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let said = stderr(&out);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 2, "{said}");
    assert!(
        lines[0].starts_with(&format!(
            "{}: invalid_input: pad d0000001 sweeps sketch s0000001, which makes no profile",
            file.display()
        )),
        "{said}"
    );
    assert!(
        lines[1].starts_with(&format!(
            "{}: worker_unavailable: SILICO_KERNEL_WORKER names",
            file.display()
        )),
        "{said}"
    );
    let out = silico_on_kernel(&scratch, &["check", "--json"], &file, &nowhere);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["ok"], false, "{line}");
    assert_eq!(line["error"]["code"], "worker_unavailable", "{line}");
    // A worker that can't be had says nothing of the feature that asked for it.
    assert_eq!(line["error"]["feature"], serde_json::Value::Null, "{line}");
    assert!(line.get("problems").is_none(), "{line}");
    // fmt --check writes nothing, saying the problem and the fault, exit 2.
    let out = silico_on_kernel(&scratch, &["fmt", "--check"], &file, &nowhere);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.contains("pad d0000001 sweeps sketch s0000001"),
        "{said}"
    );
    assert!(said.contains("not in canonical form"), "{said}");
    assert!(said.contains("worker_unavailable"), "{said}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn fmt_refused_a_save_past_the_limit_still_says_the_kernel_fault_its_rebuild_met() {
    // R2-L1: the rectangle's 40 made 40.123456, so its solve writes two corners' x as
    // that, not 40: seven bytes more each. A box after it, which needs the kernel, has a note
    // taking the document's canonical text to 5 bytes under the 16 MiB limit: it reads,
    // but the solve it writes passes the limit, so the save is refused (exit 1 on its own).
    // No kernel can be had, so the rebuild met a fault too, which the refusal's response
    // carries: the command couldn't run (exit 2), and says so after the refusal.
    let scratch = Scratch::new("past-limit-fault");
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let edited = RECTANGLE.replacen(r#""value": 40,"#, r#""value": 40.123456,"#, 1);
    assert_ne!(edited, RECTANGLE);
    let with_note = |note: &str| {
        edited.replacen(
            "\n     ]}\n  ]",
            &format!(
                "\n     ]}},\n    {{\"id\": \"k7f2qx3m\", \"type\": \"box\", \"v\": 1, \"name\": \"Block\", \"position\": [0, 0, 0], \"length\": 40, \"width\": 20, \"height\": 10, \"note\": \"{note}\"}}\n  ]"
            ),
            1,
        )
    };
    let short = canonical::canonical(&with_note("")).len();
    let text = with_note(&"x".repeat(limit - 5 - short));
    assert_eq!(canonical::canonical(&text).len(), limit - 5);
    let file = scratch.file(&text);
    let nowhere = scratch.0.join("no-kernel-worker");
    let out = silico_on_kernel(&scratch, &["fmt"], &file, &nowhere);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let said = stderr(&out);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 2, "{said}");
    assert!(lines[0].contains(&limit.to_string()), "{said}");
    assert!(
        lines[1].starts_with(&format!(
            "{}: worker_unavailable: SILICO_KERNEL_WORKER names",
            file.display()
        )),
        "{said}"
    );
    assert!(
        lines[1].ends_with("run `silico fmt` to try it again."),
        "{said}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn show_needs_no_cache_and_writes_nothing() {
    // `show` reads the document as written: it opens a session, but rebuilds nothing, so
    // it neither reads nor makes the cache's secret, and leaves no cache beside the file.
    let scratch = Scratch::new("show-writes-nothing");
    let file = scratch.file(EXAMPLE);
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .arg("show")
        .arg(&file)
        .env("SILICO_KERNEL_WORKER", "/nonexistent")
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let left: Vec<String> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["part.silico"]);
}

#[test]
fn fmt_of_a_document_the_gate_refuses_names_the_parameter_at_fault() {
    // The tests review (round 1): the document's own problems keep their parameter; only
    // open's refusal of the file drops the `path` its line already names.
    let scratch = Scratch::new("gate-parameter");
    let text = EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#);
    let file = scratch.file(&text);
    for args in [&["fmt"][..], &["fmt", "--check"], &["check"]] {
        let out = silico(&scratch, args, &file);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("got 0 (feature k7f2qx3m, parameter length). Suggestion:"),
            "{args:?}: {}",
            stderr(&out)
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "{args:?}");
    }
}

#[test]
fn a_refusal_on_stderr_doesnt_name_the_path_its_line_names() {
    // Open's refusal of a missing file: its `path` is the file the line starts with, so
    // it isn't named again as a parameter.
    let scratch = Scratch::new("missing-line");
    let file = scratch.0.join("missing.silico");
    for args in [&["check"][..], &["fmt"], &["fmt", "--check"], &["show"]] {
        let out = silico(&scratch, args, &file);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert_eq!(
            stderr(&out),
            format!(
                "{}: io_failed: couldn't be read: No such file or directory (os error 2). \
                 Suggestion: check the file's name, and that it's a regular file this user \
                 may read.\n",
                file.display()
            ),
            "{args:?}"
        );
    }
}

/// A sketch whose one line is drawn level but constrained vertical, so a solve would move
/// its end (`pb000000`, at x 10), and the example's block after it.
const SKETCH_AND_BOX: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": [
    {"id": "s0000001", "type": "sketch", "v": 1, "name": "Tilted", "plane": "xy",
     "points": [
       {"id": "pa000000", "x": 0, "y": 0, "construction": false},
       {"id": "pb000000", "x": 10, "y": 0, "construction": false}
     ],
     "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}],
     "circles": [], "arcs": [],
     "constraints": [{"id": "k0000001", "type": "vertical", "line": "na000000"}]},
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}
  ]
}
"#;

/// What the core says of a sketch worker that `SILICO_SKETCH_WORKER` names at `/nowhere`
/// (naming no feature: a worker that can't be had says nothing of the one that asked),
/// worded as the CLI words a kernel fault: its own suggestion, then to run `command` again.
fn no_sketch_worker(command: &str) -> String {
    format!(
        "worker_unavailable: SILICO_SKETCH_WORKER names \"/nowhere\", which isn't a file. \
         Suggestion: {}; then run `silico {command}` to try it again.",
        NO_SKETCH_WORKER_ADVICE
    )
}

/// The core's own suggestion for a sketch worker `SILICO_SKETCH_WORKER` names at nothing.
const NO_SKETCH_WORKER_ADVICE: &str = "point SILICO_SKETCH_WORKER at the silico-sketch-worker binary, or unset it to use the one beside Silico";

#[test]
fn fmt_around_a_fault_says_what_it_wrote_then_the_fault_last() {
    // The sketch worker can't be had: the sketch keeps what its file holds, the block's
    // fingerprint is written, and the fault is said last, in the CLI's words (run it
    // again, not a session's advice), exit 2.
    let scratch = Scratch::new("fmt-around-fault");
    let file = scratch.file(SKETCH_AND_BOX);
    let out = silico_with(&scratch, &["fmt"], &file, Path::new("/nowhere"));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let path = file.display();
    assert_eq!(
        stderr(&out),
        format!(
            "{path}: wrote the fingerprint of k7f2qx3m\n{path}: {}\n",
            no_sketch_worker("fmt")
        )
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains(r#""fingerprint": {"volume": 8000,"#),
        "{text}"
    );
    assert!(
        text.contains(r#"{"id": "pb000000", "x": 10, "y": 0, "construction": false}"#),
        "{text}"
    );
}

#[test]
fn a_kernel_fault_is_said_in_the_clis_words_by_check_and_check_json() {
    // A session's advice (an edit, or reopening the session) means nothing on the command
    // line: the worker's own suggestion, then to run it again.
    let scratch = Scratch::new("check-fault-words");
    let file = scratch.file(SKETCH_AND_BOX);
    // The block built, with no fingerprint in the file: its warning is said before the
    // fault, as the document's problems would be.
    let out = silico_with(&scratch, &["check"], &file, Path::new("/nowhere"));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let path = file.display();
    assert_eq!(
        stderr(&out),
        format!(
            "{path}: warning: fingerprint_missing: k7f2qx3m built, but its file stores no \
             fingerprint, so a change in what it builds isn't caught as drift (feature \
             k7f2qx3m, parameter fingerprint). Suggestion: format the document to store \
             it.\n{path}: {}\n",
            no_sketch_worker("check")
        )
    );
    let out = silico_with(&scratch, &["check", "--json"], &file, Path::new("/nowhere"));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    // With `--json`, the warning on stderr too; the fault is the line's.
    assert_eq!(
        stderr(&out),
        format!(
            "{path}: warning: fingerprint_missing: k7f2qx3m built, but its file stores no \
             fingerprint, so a change in what it builds isn't caught as drift (feature \
             k7f2qx3m, parameter fingerprint). Suggestion: format the document to store \
             it.\n"
        )
    );
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["ok"], false, "{line}");
    assert_eq!(line["error"]["code"], "worker_unavailable", "{line}");
    assert_eq!(
        line["error"]["suggestion"],
        format!("{NO_SKETCH_WORKER_ADVICE}; then run `silico check` to try it again"),
        "{line}"
    );
}

#[test]
fn fmt_check_says_only_that_it_would_rewrite() {
    let scratch = Scratch::new("fmt-check-only");
    let file = scratch.file(EXAMPLE);
    let out = silico(&scratch, &["fmt", "--check"], &file);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!(
            "{}: not in canonical form; `silico fmt` would rewrite it\n",
            file.display()
        )
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), EXAMPLE);
}

#[test]
fn fmt_says_wrote_for_a_fingerprint_it_had_none_of() {
    let scratch = Scratch::new("fmt-wrote");
    let file = scratch.file(EXAMPLE);
    let out = silico(&scratch, &["fmt"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!("{}: wrote the fingerprint of k7f2qx3m\n", file.display())
    );
}

#[test]
fn checking_files_with_sketches_starts_one_sketch_worker() {
    // One sketch worker for three files, not one a session: the sessions share one
    // solver, as they share the kernel pool. Each file in a directory of its own,
    // so none is solved from another's cache.
    let scratch = Scratch::new("one-sketch-worker");
    let files: Vec<PathBuf> = ["x", "y", "z"]
        .iter()
        .map(|dir| {
            std::fs::create_dir(scratch.0.join(dir)).unwrap();
            let file = scratch.0.join(dir).join("part.silico");
            std::fs::write(&file, RECTANGLE).unwrap();
            file
        })
        .collect();
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.arg("check")
        .args(&files)
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
    assert_eq!(stderr(&out).matches("started sketch worker").count(), 1);
}

/// `silico ARGS FILE` on the real workers, logging their starts: its output, and how many
/// kernel workers it started.
fn run_counting_workers(scratch: &Scratch, args: &[&str], file: &Path) -> (Output, usize) {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .arg(file)
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
    let started = stderr(&out).matches("started kernel worker").count();
    (out, started)
}

#[test]
fn check_fresh_rebuilds_from_the_kernel_with_a_full_cache_and_leaves_it_full() {
    // M4.3: `--fresh` reads nothing of the cache, so the block is made on a worker though
    // its entry is there; and what it builds is kept, so a plain check after starts none.
    let scratch = Scratch::new("check-fresh");
    let file = scratch.file(EXAMPLE);
    let (out, started) = run_counting_workers(&scratch, &["check"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 1),
        "{}",
        stderr(&out)
    );
    let (out, started) = run_counting_workers(&scratch, &["check"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 0),
        "{}",
        stderr(&out)
    );
    let (out, started) = run_counting_workers(&scratch, &["check", "--fresh"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 1),
        "{}",
        stderr(&out)
    );
    let (out, started) = run_counting_workers(&scratch, &["check"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 0),
        "{}",
        stderr(&out)
    );
    // With `--json`, the same line as a plain check's, in either order of the options.
    for args in [
        &["check", "--fresh", "--json"][..],
        &["check", "--json", "--fresh"],
    ] {
        let out = silico(&scratch, args, &file);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            line,
            serde_json::json!({"file": file.display().to_string(), "ok": true, "problems": [], "total": 0})
        );
    }
    // A document with problems is exit 1, fresh or not; a misspelt option is a usage error.
    let bad = scratch.file(&EXAMPLE.replace(r#""length": 40"#, r#""length": 0"#));
    assert_eq!(
        silico(&scratch, &["check", "--fresh"], &bad).status.code(),
        Some(1)
    );
    let out = silico(&scratch, &["check", "--frseh"], &bad);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).starts_with("silico check: no option \"--frseh\"\n"),
        "{}",
        stderr(&out)
    );
    // `fmt` takes no `--fresh`.
    assert_eq!(
        silico(&scratch, &["fmt", "--fresh"], &bad).status.code(),
        Some(2)
    );
}

#[test]
fn cache_clear_removes_the_entries_beside_a_file_and_says_how_many() {
    let scratch = Scratch::new("cache-clear");
    let file = scratch.file(EXAMPLE);
    let (out, started) = run_counting_workers(&scratch, &["check"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 1),
        "{}",
        stderr(&out)
    );
    let cache = scratch.0.join(".silico-cache");
    let stray = cache.join("notes.txt");
    std::fs::write(&stray, b"mine").unwrap();
    // No kernel is needed, nor the cache's secret (none is made where it would be, as
    // `show` makes none): the block's one entry removed. Run as it is, not through
    // `run::output`, which gives each run a secret.
    let clear = || {
        Command::new(env!("CARGO_BIN_EXE_silico"))
            .args(["cache", "clear"])
            .arg(&file)
            .env("SILICO_KERNEL_WORKER", "/nonexistent")
            .env("XDG_CACHE_HOME", scratch.0.join("no-cache-home"))
            .env_remove("SILICO_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = clear();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!(
            "{}: removed 1 entry from {}\n",
            file.display(),
            cache.display()
        )
    );
    assert!(!scratch.0.join("no-cache-home").exists());
    assert_eq!(std::fs::read(&stray).unwrap(), b"mine");
    let out = clear();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!(
            "{}: removed 0 entries from {}\n",
            file.display(),
            cache.display()
        )
    );
    // A miss: the next check makes the block again.
    let (out, started) = run_counting_workers(&scratch, &["check"], &file);
    assert_eq!(
        (out.status.code(), started),
        (Some(0), 1),
        "{}",
        stderr(&out)
    );
}

#[test]
fn cache_without_clear_or_a_file_is_a_usage_error_and_a_file_it_cant_read_exit_2() {
    let scratch = Scratch::new("cache-usage");
    let file = scratch.file(EXAMPLE);
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_silico"));
        cmd.args(args)
            .current_dir(&scratch.0)
            .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
            .env_remove("SILICO_LOG")
            .stdin(Stdio::null());
        run::output(cmd)
    };
    for (args, said) in [
        (&["cache"][..], "silico cache: which? clear\n"),
        (
            &["cache", "wipe", "part.silico"],
            "silico cache: no subcommand \"wipe\"\n",
        ),
        (
            &["cache", "--all", "clear", "part.silico"],
            "silico cache: no option \"--all\"\n",
        ),
        (&["cache", "clear"], "usage: "),
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).starts_with(said), "{args:?}: {}", stderr(&out));
    }
    // `clear` and then `--` ends the options, so a file may be named like one.
    let dashed = scratch.0.join("--all.silico");
    std::fs::write(&dashed, EXAMPLE).unwrap();
    let out = run(&["cache", "clear", "--", "--all.silico"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // A file that isn't there: exit 2, as check's.
    let out = run(&["cache", "clear", "missing.silico"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("io_failed"), "{}", stderr(&out));
    // A cache whose entry can't be removed: said, exit 2.
    let out = silico(&scratch, &["check"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let cache = scratch.0.join(".silico-cache");
    let sub = entries(&cache)[0].parent().unwrap().to_owned();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(sub.join("probe"), b"").is_err() {
        let out = run(&["cache", "clear", "part.silico"]);
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("io_failed: couldn't remove the cache's entry"),
            "{}",
            stderr(&out)
        );
    }
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn cache_clear_of_several_files_says_each_and_exits_with_the_worst() {
    // Two documents, each in a directory of its own, each with a cache of one entry (the
    // block's) beside it.
    let scratch = Scratch::new("cache-clear-several");
    let dirs = [scratch.0.join("one"), scratch.0.join("two")];
    let files: Vec<PathBuf> = dirs
        .iter()
        .map(|dir| {
            std::fs::create_dir_all(dir).unwrap();
            let file = dir.join("part.silico");
            std::fs::write(&file, EXAMPLE).unwrap();
            file
        })
        .collect();
    let caches: Vec<PathBuf> = dirs.iter().map(|dir| dir.join(".silico-cache")).collect();
    let check_all = || {
        for file in &files {
            let out = silico(&scratch, &["check"], file);
            assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        }
    };
    // The files cleared in the order `order` names them.
    let clear = |order: [usize; 2]| {
        Command::new(env!("CARGO_BIN_EXE_silico"))
            .args(["cache", "clear"])
            .args(order.map(|i| &files[i]))
            .env("SILICO_KERNEL_WORKER", "/nonexistent")
            .env("XDG_CACHE_HOME", scratch.0.join("no-cache-home"))
            .env_remove("SILICO_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // Both cleared: a line each, in the order named, exit 0.
    check_all();
    let out = clear([0, 1]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!(
            "{}: removed 1 entry from {}\n{}: removed 1 entry from {}\n",
            files[0].display(),
            caches[0].display(),
            files[1].display(),
            caches[1].display()
        )
    );
    // The first's entry can't be removed: said, the second cleared all the same, and the
    // run couldn't do all it was asked (exit 2), whichever comes first: named first, so a
    // run that took the last file's exit would exit 0; or named last, so one that took the
    // first's would.
    let failing = |file: usize, entry: &Path| {
        format!(
            "{}: io_failed: couldn't remove the cache's entry \"{}\": Permission denied (os \
             error 13): the clear removed 0 entries and left what it couldn't remove or read. \
             Suggestion: check that this user may read and write the cache's directory and \
             its subdirectories, then clear it again: an entry left is still sound, and one \
             removed is only a miss.\n",
            files[file].display(),
            entry.display(),
        )
    };
    let cleared = |file: usize| {
        format!(
            "{}: removed 1 entry from {}\n",
            files[file].display(),
            caches[file].display()
        )
    };
    for order in [[0, 1], [1, 0]] {
        check_all();
        let first = entries(&caches[0]);
        assert_eq!(first.len(), 1, "{first:?}");
        let entry = &first[0];
        let sub = entry.parent().unwrap();
        std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::write(sub.join("probe"), b"").is_ok() {
            return; // root
        }
        let out = clear(order);
        std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(out.status.code(), Some(2), "{order:?}: {}", stderr(&out));
        // What's said of each file (the cache's warning, logged at the default level, aside).
        let told: String = stderr(&out)
            .lines()
            .filter(|line| !line.contains(" WARN silico_core::cache: "))
            .map(|line| format!("{line}\n"))
            .collect();
        let said: String = order
            .map(|file| match file {
                0 => failing(0, entry),
                _ => cleared(1),
            })
            .concat();
        assert_eq!(told, said, "{order:?}");
        assert!(entry.is_file());
        assert_eq!(entries(&caches[1]), Vec::<PathBuf>::new());
    }
}

/// A kernel worker that says it started (an empty frame on its socket, descriptor 3), takes
/// a byte of the first request, and then runs `end`: how a worker's crash during a request
/// is met, without a real crash of OCCT.
fn worker_that_ends(scratch: &Scratch, name: &str, end: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch.0.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '\\0\\0\\0\\0' >&3\nhead -c 1 <&3 >/dev/null\n{end}\nexec sleep 600\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn each_crash_says_the_designs_words_once_and_a_values_crash_only_to_change_them() {
    // The design's section 6 (docs/design/m4.1-command-api.md), as docs/cli.md's rows point
    // to it: the CLI words a fault as a session does, trying again by running its command
    // ("run `silico check` to try it again"), said once. A worker stopped from outside: "run
    // `silico check` to try it again: the kernel worker was stopped from outside, stopped
    // responding, or was gone before the request reached it, not by these values". A crash on
    // the request's values: the worker's advice alone, "change the operation's values: these
    // crashed the kernel, and the same request would crash it again". Any other advice of the
    // worker's own (its install, for one that exits), it, then "; then run `silico check` to
    // try it again". A kill, which may have been for the memory, is "once more".
    let scratch = Scratch::new("crash-advice");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, EXAMPLE).unwrap();
    let values = "change the operation's values: these crashed the kernel, and the same request \
                  would crash it again";
    let outside = |command: &str| {
        format!(
            "run `silico {command}` to try it again: the kernel worker was stopped from outside, \
             stopped responding, or was gone before the request reached it, not by these values"
        )
    };
    let terminated = outside("check");
    // Each worker's end, and its suggestion: how it starts and ends (the whole of it where
    // the two are the same).
    let cases = [
        ("segfault", "kill -SEGV $$", values, values),
        (
            "terminated",
            "kill -TERM $$",
            terminated.as_str(),
            terminated.as_str(),
        ),
        (
            "killed",
            "kill -KILL $$",
            "run `silico check` to try it once more: the kernel worker was killed",
            "the request needs too much memory: simplify it",
        ),
        (
            "exited",
            "exit 1",
            "check Silico's install: ",
            "; then run `silico check` to try it again",
        ),
    ];
    for (name, end, opening, ending) in cases {
        let kernel = worker_that_ends(&scratch, name, end);
        let out = silico_on_kernel(&scratch, &["check", "--json"], &file, &kernel);
        assert_eq!(out.status.code(), Some(2), "{name}: {}", stderr(&out));
        let line: serde_json::Value =
            serde_json::from_slice(out.stdout.split(|&b| b == b'\n').next().unwrap()).unwrap();
        assert_eq!(line["ok"], false, "{name}: {line}");
        assert_eq!(line["error"]["code"], "worker_crashed", "{name}: {line}");
        let suggestion = line["error"]["suggestion"].as_str().unwrap();
        assert!(suggestion.starts_with(opening), "{name}: {suggestion:?}");
        assert!(suggestion.ends_with(ending), "{name}: {suggestion:?}");
        if opening == ending {
            assert_eq!(suggestion, opening, "{name}");
        }
        // How to try again, said once, or not at all after the values' crash; never the
        // more time no option gives.
        let tries = usize::from(name != "segfault");
        assert_eq!(
            suggestion.matches("to try it").count(),
            tries,
            "{name}: {suggestion:?}"
        );
        assert!(!suggestion.contains("more time"), "{name}: {suggestion:?}");
        // Without --json, the same suggestion, said once.
        let out = silico_on_kernel(&scratch, &["check"], &file, &kernel);
        assert_eq!(out.status.code(), Some(2), "{name}: {}", stderr(&out));
        let said = stderr(&out);
        assert_eq!(said.matches(suggestion).count(), 1, "{name}: {said}");
        assert_eq!(said.matches("to try it").count(), tries, "{name}: {said}");
    }
    // fmt and export word a kernel fault as check does, each naming itself (docs/cli.md,
    // their rows).
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let out = scratch.0.join("part.3mf");
    for (name, end) in [
        ("segfault", "kill -SEGV $$"),
        ("terminated", "kill -TERM $$"),
    ] {
        let kernel = worker_that_ends(&scratch, name, end);
        for (command, args) in [
            ("fmt", vec![file.clone()]),
            ("export", vec![file.clone(), out.clone()]),
        ] {
            let mut cmd = Command::new(exe);
            cmd.arg(command)
                .args(&args)
                .env("SILICO_KERNEL_WORKER", &kernel)
                .env(
                    "SILICO_SKETCH_WORKER",
                    exe.with_file_name("silico-sketch-worker"),
                )
                .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
                .env_remove("SILICO_LOG")
                .stdin(Stdio::null());
            let ran = run::output(cmd);
            let said = stderr(&ran);
            assert_eq!(ran.status.code(), Some(2), "{command} {name}: {said}");
            let expected = if name == "segfault" {
                values.to_owned()
            } else {
                outside(command)
            };
            assert_eq!(
                said.matches(&expected).count(),
                1,
                "{command} {name}: {said}"
            );
            let tries = usize::from(name != "segfault");
            assert_eq!(
                said.matches("to try it").count(),
                tries,
                "{command} {name}: {said}"
            );
        }
    }
    assert!(!out.exists());
}
