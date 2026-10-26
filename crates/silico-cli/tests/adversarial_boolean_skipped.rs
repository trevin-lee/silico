//! break-it for PR #171 (a boolean whose OCCT skipped intersecting a pair of elements is
//! refused): a document a user could write, a pointed cone with a padded rod resting on its
//! tip, cut. The rod touches the cone at one point and nowhere else, so the cut is the cone
//! whole: 125π/3 = 130.899694 (by hand; FreeCAD 1.1.3, on the same OCCT, gives 130.8996939
//! for the same solids). Exit codes from docs/cli.md: 0 clean, 1 the document has problems.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ROD_ON_THE_POINT: &str = r#"
{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {
      "id": "kkkkkkkk",
      "type": "cone",
      "v": 1,
      "name": "Point",
      "position": [0, 0, 0],
      "radius1": 5,
      "radius2": 0,
      "height": 5
    },
    {
      "id": "s0000001",
      "type": "sketch",
      "v": 1,
      "name": "Rod section",
      "plane": "xz",
      "points": [
        {"id": "p0000001", "x": 0, "y": 7, "construction": false}
      ],
      "lines": [],
      "circles": [
        {"id": "c0000001", "center": "p0000001", "radius": 2, "construction": false}
      ],
      "arcs": [],
      "constraints": []
    },
    {
      "id": "rrrrrrrr",
      "type": "pad",
      "v": 1,
      "name": "Rod",
      "sketch": "s0000001",
      "extent": {"type": "symmetric", "length": 20}
    },
    {
      "id": "hhhhhhhh",
      "type": "cut",
      "v": 1,
      "name": "Rod on the point",
      "base": "kkkkkkkk",
      "tool": "rrrrrrrr"
    }
  ]
}
"#;

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("adv-skipped-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_rod_padded_onto_a_pointed_cones_tip_cuts_and_checks_clean() {
    // BREAKS: `silico check` exits 1: "kernel_failed: Cut failed (feature hhhhhhhh, element
    // Face1): OCCT skipped intersecting the base's Face1 with the tool's face 1 in the cut",
    // for a cut whose right result is the cone whole, which `fmt` would then fingerprint.
    let scratch = Scratch::new("rod-on-point");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, ROD_ON_THE_POINT.trim_start()).unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.arg("fmt")
        .arg(&file)
        .env(
            "SILICO_KERNEL_WORKER",
            exe.with_file_name("silico-kernel-worker"),
        )
        .env(
            "SILICO_SKETCH_WORKER",
            exe.with_file_name("silico-sketch-worker"),
        )
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    let out = run::output(cmd);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let text = std::fs::read_to_string(&file).unwrap();
    // The cut's fingerprint: the cone's volume, 125π/3, to the format's six places.
    let cut = text.split("\"id\": \"hhhhhhhh\"").nth(1).unwrap();
    assert!(
        cut.contains(r#""fingerprint": {"volume": 130.899694,"#),
        "{text}"
    );
}
