//! M0.5: `silico` logs through `tracing` to stderr (never stdout), filtered by SILICO_LOG,
//! as text or, with SILICO_LOG_FORMAT=json, one JSON object per line. Each test runs
//! `silico show`, whose stdout is the outline alone and whose stderr is empty but for logs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output, Stdio};

fn run(env: &[(&str, &str)]) -> Output {
    run_os(
        &env.iter()
            .map(|(k, v)| (*k, std::ffi::OsString::from(v)))
            .collect::<Vec<_>>(),
    )
}

/// A one-box document, and its outline.
const DOCUMENT: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
const OUTLINE: &str =
    "k7f2qx3m \"Block\" box length 40, width 20, height 10, at (0, 0, 0)\nbodies k7f2qx3m\n";

/// `silico show` of [`DOCUMENT`], which writes the outline to stdout and nothing to
/// stderr: anything else on stderr is a log line, and stdout must be the outline alone.
fn run_os(env: &[(&str, std::ffi::OsString)]) -> Output {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let file = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "logging-{}-{}.silico",
        std::process::id(),
        RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&file, DOCUMENT).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_silico"));
    cmd.arg("show")
        .arg(&file)
        .stdin(Stdio::null())
        .env_remove("SILICO_LOG")
        .env_remove("SILICO_LOG_FORMAT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let _ = std::fs::remove_file(&file);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        OUTLINE,
        "logs leaked to stdout"
    );
    out
}

#[test]
fn quiet_by_default() {
    let out = run(&[]);
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn json_logs_are_one_object_per_line() {
    let out = run(&[("SILICO_LOG", "debug"), ("SILICO_LOG_FORMAT", "json")]);
    let stderr = String::from_utf8(out.stderr).unwrap();
    let events: Vec<serde_json::Value> = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    let start = events
        .iter()
        .find(|e| e["fields"]["message"] == "silico starting")
        .unwrap_or_else(|| panic!("no start event in {stderr}"));
    assert_eq!(start["level"], "DEBUG");
    assert!(start["fields"]["version"].is_string(), "{start}");
}

#[test]
fn text_logs_by_default_format() {
    for format in [None, Some("text"), Some("")] {
        let mut env = vec![("SILICO_LOG", "debug")];
        env.extend(format.map(|f| ("SILICO_LOG_FORMAT", f)));
        let out = run(&env);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains("silico starting"), "{format:?}: {stderr}");
        assert!(
            !stderr.trim_start().starts_with('{'),
            "{format:?}: {stderr}"
        );
        assert!(
            !stderr.contains("ignoring"),
            "{format:?} was rejected: {stderr}"
        );
        // stderr isn't a terminal here, so no colour codes.
        assert!(!stderr.contains('\x1b'), "{format:?}: {stderr:?}");
    }
}

#[test]
fn unknown_format_warns_and_uses_text() {
    let out = run(&[("SILICO_LOG_FORMAT", "JSON")]);
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("SILICO_LOG_FORMAT"), "{stderr}");
    assert!(!stderr.trim_start().starts_with('{'), "{stderr}");
}

#[test]
fn empty_filter_means_the_default() {
    // The default (warn) still shows warnings, such as the one for an unknown format.
    let out = run(&[("SILICO_LOG", ""), ("SILICO_LOG_FORMAT", "bogus")]);
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("SILICO_LOG_FORMAT"), "{stderr}");
}

#[test]
fn non_utf8_filter_warns() {
    use std::os::unix::ffi::OsStringExt;
    let out = run_os(&[("SILICO_LOG", std::ffi::OsString::from_vec(vec![0xff]))]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not UTF-8"), "{stderr}");
}

#[test]
fn invalid_filter_warns_and_keeps_going() {
    let out = run(&[("SILICO_LOG", "silico=notalevel")]);
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("SILICO_LOG"), "{stderr}");
}
