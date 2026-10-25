//! Records where the pixi environment the crate is built in keeps Mesa's lavapipe and the
//! Vulkan loader, so the render worker finds them without any ambient environment
//! (docs/design/m4.6-render.md, *How the worker gets its rasterizer*).

use std::io::Write;
use std::path::{Path, PathBuf};

fn main() -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "cargo:rerun-if-env-changed=CONDA_PREFIX")?;
    let prefix = std::env::var_os("CONDA_PREFIX").map(PathBuf::from);
    let icd = prefix.as_deref().and_then(lavapipe_icd);
    let lib = prefix.as_ref().map(|p| p.join("lib"));
    // Installing or removing lavapipe in the same environment changes what's found.
    if let Some(dir) = prefix
        .as_ref()
        .map(|p| p.join("share/vulkan/icd.d"))
        .filter(|d| d.is_dir())
    {
        writeln!(out, "cargo:rerun-if-changed={}", dir.display())?;
    }
    // Empty when built outside pixi: the worker then has no lavapipe, and says so.
    let shown = |p: Option<PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_default();
    writeln!(out, "cargo:rustc-env=SILICO_LAVAPIPE_ICD={}", shown(icd))?;
    writeln!(out, "cargo:rustc-env=SILICO_VULKAN_LIB={}", shown(lib))?;
    Ok(())
}

/// The environment's lavapipe ICD file (`share/vulkan/icd.d/lvp_icd.<arch>.json`), the first
/// by name if there are several.
fn lavapipe_icd(prefix: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(prefix.join("share/vulkan/icd.d"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("lvp_icd.") && n.ends_with(".json"))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}
