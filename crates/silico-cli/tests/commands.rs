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
    for args in [&["chekc"][..], &["check", "--jsno"], &["fmt", "--json"]] {
        let out = silico(&scratch, args, &file);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).contains("usage: silico"), "{}", stderr(&out));
    }
}
