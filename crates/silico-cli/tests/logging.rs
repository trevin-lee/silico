//! M0.5: `silico` logs through `tracing` to stderr (never stdout), filtered by SILICO_LOG,
//! as text or, with SILICO_LOG_FORMAT=json, one JSON object per line.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output, Stdio};

fn run(env: &[(&str, &str)]) -> Output {
    run_os(
        &env.iter()
            .map(|(k, v)| (*k, std::ffi::OsString::from(v)))
            .collect::<Vec<_>>(),
    )
}

fn run_os(env: &[(&str, std::ffi::OsString)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_silico"));
    cmd.stdin(Stdio::null())
        .env_remove("SILICO_LOG")
        .env_remove("SILICO_LOG_FORMAT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "logs leaked to stdout: {out:?}");
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
