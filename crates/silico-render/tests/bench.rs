//! The render bench's program: it restarts itself with lavapipe as the worker does (the
//! library's `lavapipe::reexec_with`, which the worker shares), refuses a restart that lost
//! a variable rather than restarting again, refuses arguments it can't use, and renders.

use std::collections::BTreeMap;
use std::process::{Command, Output};

/// The bench run with `args`, its environment cleared, as the core's is for every worker.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_silico-render-bench"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .unwrap()
}

#[test]
fn the_bench_renders_on_lavapipe_from_a_cleared_environment() {
    let out = run(&["64", "--hold", "0"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let found: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once(' ')).collect();
    assert!(found["adapter"].starts_with("llvmpipe"), "{found:?}");
    assert_eq!(found["size"], "64", "{found:?}");
    assert!(found["png_bytes"].parse::<u64>().unwrap() > 0, "{found:?}");
    assert!(found.contains_key("memory_after_drop"), "{found:?}");
}

#[test]
fn a_restart_that_lost_lavapipes_variables_fails_naming_one_rather_than_restarting_again() {
    // As if the restart's variables were stripped on the way. macOS strips only DYLD_*
    // from a child of a protected program, so the others arrive and the loader's path is
    // the one named; with all of them gone, the first is.
    let library_path = if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let kept = [
        ("VK_ICD_FILENAMES", silico_render::lavapipe::ICD),
        ("LP_NUM_THREADS", silico_render::lavapipe::THREADS),
    ];
    for (arrived, missing) in [(&kept[..], library_path), (&[][..], "VK_ICD_FILENAMES")] {
        let out = Command::new(env!("CARGO_BIN_EXE_silico-render-bench"))
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(arrived.iter().copied())
            .env("SILICO_RENDER_RESTARTED", "1")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{missing}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!(
                "silico-render-bench: restarted with lavapipe's variables, but {missing} \
                 didn't reach the restart\n"
            )
        );
        assert!(out.stdout.is_empty(), "{missing}");
    }
}

#[test]
fn the_bench_refuses_arguments_it_cant_use_saying_why() {
    for (args, says) in [
        (&["0"][..], "\"0\" isn't a size from 1 to 4096 pixels"),
        (&["4097"], "\"4097\" isn't a size from 1 to 4096 pixels"),
        (&["abc"], "\"abc\" isn't a size from 1 to 4096 pixels"),
        (&["400", "--hold"], "--hold takes a number of seconds"),
        (&["--hold"], "--hold takes a number of seconds"),
        (
            &["400", "--hold", "soon"],
            "--hold takes a number of seconds",
        ),
        (&["400", "500"], "unexpected arguments: [\"500\"]"),
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("silico-render-bench: {says}\n"),
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}
