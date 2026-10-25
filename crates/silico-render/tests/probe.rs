//! M4.6's first step on this machine: the box's front, rendered headless on lavapipe, has the
//! front face's ID over exactly the pixels whose centres its projection covers by hand, and
//! the background everywhere else, at the smallest size, a size whose rows the readback pads,
//! a common one, and the largest the render takes (docs/design/m4.6-render.md: Build steps,
//! 1; R5's boundary rule). The face is off the image's centre both ways, so a flipped or
//! mirrored image fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::process::{Command, Output};

/// The probe run with `args`, its environment cleared, as the core's is for every worker:
/// lavapipe comes from the paths compiled in, and the program's own restart.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_silico-render-probe"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .unwrap()
}

fn probe(size: u32) -> BTreeMap<String, String> {
    let out = run(&[&size.to_string()]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// The pixels, from 0, whose centres lie strictly between `low` and `high` (in pixels).
fn covered(low: f64, high: f64, size: u32) -> Vec<u32> {
    (0..size)
        .filter(|&i| {
            let centre = f64::from(i) + 0.5;
            low < centre && centre < high
        })
        .collect()
}

#[test]
fn the_box_from_the_front_shows_its_front_face_over_its_projection_on_lavapipe() {
    for size in [1, 400, 401, 4096] {
        let found = probe(size);
        assert!(found["adapter"].starts_with("llvmpipe"), "{found:?}");
        assert_eq!(found["size"], size.to_string(), "{found:?}");
        // By hand: the view is 44 mm high about the image's centre, and the front face spans
        // x from -19 to 21 mm and z from -3 to 7 mm about it, z up and rows down.
        let pixel = f64::from(size) / 44.0;
        let half = f64::from(size) / 2.0;
        let cols = covered(half - 19.0 * pixel, half + 21.0 * pixel, size);
        let rows = covered(half - 7.0 * pixel, half + 3.0 * pixel, size);
        // Every edge of the face is in the image, background beyond it, so each is tested
        // (a single pixel has no edges to show).
        if size > 1 {
            for span in [&cols, &rows] {
                assert!(
                    span[0] > 0 && span[span.len() - 1] < size - 1,
                    "{size}: {span:?}"
                );
            }
        }
        let face = u64::try_from(cols.len() * rows.len()).unwrap();
        let background = u64::from(size) * u64::from(size) - face;
        // Each ID seen, with its pixels: none with no pixels.
        let ids: Vec<String> = [("0:0", background), ("1:3", face)]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(id, n)| format!("{id}={n}"))
            .collect();
        assert_eq!(found["ids"], ids.join(","), "{found:?}");
        let front = format!(
            "{},{},{},{}",
            cols[0],
            cols[cols.len() - 1],
            rows[0],
            rows[rows.len() - 1]
        );
        assert_eq!(found["front"], front, "{found:?}");
        assert_eq!(found["off"], "0", "{found:?}");
    }
}

#[test]
fn a_restart_that_lost_lavapipes_variables_fails_naming_one_rather_than_restarting_again() {
    // As if the restart's variables were stripped on the way (macOS strips DYLD_* from a
    // child of a protected program): the marker is there, the variables aren't.
    let out = Command::new(env!("CARGO_BIN_EXE_silico-render-probe"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("SILICO_RENDER_RESTARTED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "silico-render-probe: restarted with lavapipe's variables, but VK_ICD_FILENAMES \
         didn't reach the restart\n"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn the_probe_refuses_arguments_it_cant_use_saying_why() {
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
            format!("silico-render-probe: {says}\n"),
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}
