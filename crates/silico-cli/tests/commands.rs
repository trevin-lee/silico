//! M2.9: `silico check`, `fmt`, and `show` as a user or a script runs them, on the real
//! kernel worker (built beside `silico` when the workspace is built, as `pixi run test`
//! builds it). Expected values are written by hand: the example's measures (a 40 × 20 × 10
//! block: volume 8000), exit codes, and output shapes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

/// `silico ARGS` with the kernel worker beside it and the cache's secret in `scratch`.
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
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let started = |files: &[&str]| -> usize {
        let out = Command::new(exe)
            .arg("check")
            .args(files.iter().map(|f| scratch.0.join(f)))
            .env(
                "SILICO_KERNEL_WORKER",
                exe.with_file_name("silico-kernel-worker"),
            )
            .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
            .env("SILICO_LOG", "debug")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        stderr(&out).matches("started kernel worker").count()
    };
    std::fs::create_dir(scratch.0.join("x")).unwrap();
    std::fs::create_dir(scratch.0.join("y")).unwrap();
    for name in ["x/a.silico", "x/b.silico", "y/c.silico", "y/d.silico"] {
        std::fs::write(scratch.0.join(name), EXAMPLE).unwrap();
    }
    let one = started(&["x/a.silico"]);
    assert!(one > 0);
    assert_eq!(started(&["x/a.silico", "x/b.silico"]), one);
    // Directories in turn, each with its own cache, on the one pool (the review found a
    // pool started at every change of directory).
    assert_eq!(
        started(&["x/a.silico", "y/c.silico", "x/b.silico", "y/d.silico"]),
        one
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
    let out = Command::new(env!("CARGO_BIN_EXE_silico"))
        .args(["check", "--json", "bad.silico", "missing.silico"])
        .current_dir(&scratch.0)
        .env("SILICO_KERNEL_WORKER", "/nonexistent")
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null())
        .output()
        .unwrap();
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
