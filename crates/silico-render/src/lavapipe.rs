//! Where the build found Mesa's lavapipe, and a process's restart with it.

use std::os::unix::process::CommandExt;
use std::path::Path;

/// Lavapipe's ICD file in the pixi environment the crate was built in; empty if none.
pub const ICD: &str = env!("SILICO_LAVAPIPE_ICD");
/// The directory holding that environment's Vulkan loader (`libvulkan`); empty if none.
pub const VULKAN_LIB: &str = env!("SILICO_VULKAN_LIB");
/// llvmpipe's threads: renders are small, and the machine's other work needs its cores.
pub const THREADS: &str = "2";

/// Set in the restarted process, so a restart whose variables didn't all survive (macOS
/// strips `DYLD_*` from children of its protected programs) fails instead of restarting
/// forever.
const RESTARTED: &str = "SILICO_RENDER_RESTARTED";

/// The library path variable the loader is found through on this platform.
const LIBRARY_PATH: &str = if cfg!(target_os = "macos") {
    "DYLD_FALLBACK_LIBRARY_PATH"
} else {
    "LD_LIBRARY_PATH"
};

/// The variables a render process needs for lavapipe, if the compiled-in ICD exists. None
/// otherwise, so a machine's own Vulkan drivers stay visible.
pub fn environment() -> Option<[(&'static str, &'static str); 3]> {
    (!ICD.is_empty() && Path::new(ICD).is_file()).then_some([
        ("VK_ICD_FILENAMES", ICD),
        (LIBRARY_PATH, VULKAN_LIB),
        ("LP_NUM_THREADS", THREADS),
    ])
}

/// Re-executes this process with lavapipe's variables unless it already has them, before
/// anything touches `wgpu`. Returns at once if it has them or there's no lavapipe; otherwise
/// returns only if the exec fails, or if this is the restart and a variable didn't survive
/// it, with why. Descriptors without close-on-exec (a worker's inherited socket) survive the
/// exec.
pub fn reexec_with() -> Result<(), String> {
    let Some(vars) = environment() else {
        return Ok(());
    };
    let missing = vars
        .iter()
        .find(|(k, v)| std::env::var(k).map_or(true, |have| have != *v));
    let Some((missing, _)) = missing else {
        return Ok(());
    };
    if std::env::var_os(RESTARTED).is_some() {
        return Err(format!(
            "restarted with lavapipe's variables, but {missing} didn't reach the restart"
        ));
    }
    let exe = std::env::current_exe().map_err(|e| format!("can't find this program: {e}"))?;
    let error = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .envs(vars)
        .env(RESTARTED, "1")
        .exec();
    Err(format!(
        "couldn't restart {} with lavapipe: {error}",
        exe.display()
    ))
}
