//! A client's workers, found before it starts (the M4 close, overturning #199): `silico
//! serve` and the app each start with all four, kernel, sketch, render, and STEP, or not at
//! all, saying each one missing and where it looked, so a broken install is met at the
//! start rather than at the command that first needs the missing worker. One found that
//! can't run is met when it's first asked, as its fault.

use crate::pool::KernelPool;
use crate::render::{RenderClient, RenderSettings};

/// The workers a client starts with: the kernel pool and the render worker's client (the
/// sketch and STEP workers are started by the commands that need them, from where they
/// were found here).
pub struct Workers {
    pub kernel: KernelPool,
    pub renderer: RenderClient,
}

/// Every worker found, or why each that wasn't: one line for each, the kernel's first,
/// then the sketch, STEP, and render workers', each saying what nothing can be done without
/// it and where it was looked for.
pub fn find() -> Result<Workers, Vec<String>> {
    let kernel = KernelPool::start();
    let mut missing = Vec::new();
    if let Some(why) = kernel.refused() {
        missing.push(format!(
            "there's no kernel worker it can use, so nothing can be built: {why}"
        ));
    }
    if let Err(e) = crate::sketch::worker_path() {
        missing.push(format!(
            "there's no sketch worker, so no sketch can be solved: {e}"
        ));
    }
    if let Err(e) = crate::step_worker::worker_path() {
        missing.push(format!(
            "there's no STEP worker, so nothing can be exported as STEP: {e}"
        ));
    }
    let renderer = crate::render::worker_path()
        .and_then(|exe| RenderClient::start(&exe, RenderSettings::default()));
    let renderer = match renderer {
        Ok(renderer) => Some(renderer),
        Err(e) => {
            missing.push(format!(
                "there's no render worker, so nothing can be rendered: {e}"
            ));
            None
        }
    };
    match (renderer, missing.is_empty()) {
        (Some(renderer), true) => Ok(Workers { kernel, renderer }),
        _ => Err(missing),
    }
}
