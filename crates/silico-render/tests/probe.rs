//! M4.6's first step on this machine: the box's front, rendered headless on lavapipe, has the
//! front face's ID over exactly the rectangle its projection covers by hand (within a pixel
//! of its edges) and the background beyond it, at the smallest useful size and the largest
//! the render takes (docs/design/m4.6-render.md: Build steps, 1; R5's boundary rule).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::process::Command;

fn probe(size: u32) -> BTreeMap<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_silico-render-probe"))
        .arg(size.to_string())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .unwrap();
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

#[test]
fn the_box_from_the_front_shows_its_front_face_over_its_projection_on_lavapipe() {
    // The worker's environment is cleared, as the core's is for every worker: lavapipe comes
    // from the paths compiled in, and the program's own restart.
    for size in [400, 4096] {
        let found = probe(size);
        assert!(found["adapter"].starts_with("llvmpipe"), "{found:?}");
        assert_eq!(found["off"], "0", "{found:?}");
        // Only the front face (body 1, Face3) and the background are seen.
        let seen: Vec<&str> = found["ids"]
            .split(',')
            .map(|e| e.split('=').next().unwrap())
            .collect();
        assert_eq!(seen, ["0:0", "1:3"], "{found:?}");
    }
}
