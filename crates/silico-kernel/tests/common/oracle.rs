//! The FreeCAD oracle (minimal; M1 grows it into `silico-oracle`): runs a FreeCAD Python
//! script with `freecadcmd` from the pixi `oracle` environment and returns the JSON the
//! script prints on a line starting `SILICO_ORACLE `. FreeCAD uses its own OCCT build, so
//! comparisons use tolerances (ARCHITECTURE.md).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SCRIPTS: AtomicU32 = AtomicU32::new(0);

/// `freecadcmd` from the pixi `oracle` environment. A missing oracle is a failure, never a
/// skip: a test that quietly didn't compare against FreeCAD would look like one that did.
fn freecadcmd() -> PathBuf {
    let exe = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.pixi/envs/oracle/bin/freecadcmd"
    ));
    assert!(
        exe.exists(),
        "the FreeCAD oracle isn't installed: run `pixi install -e oracle` ({} is missing)",
        exe.display()
    );
    exe
}

/// Runs `script` (Python, with FreeCAD's modules importable) and returns the value it
/// printed as `print("SILICO_ORACLE " + json.dumps(value))`.
pub fn freecad(script: &str) -> serde_json::Value {
    let n = SCRIPTS.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oracle-{}-{n}.py", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = Command::new(freecadcmd()).arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("SILICO_ORACLE "))
        .unwrap_or_else(|| {
            panic!(
                "FreeCAD printed no result ({}):\n{stdout}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )
        });
    serde_json::from_str(line).unwrap()
}
