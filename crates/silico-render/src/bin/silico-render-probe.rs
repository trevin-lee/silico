//! M4.6's first step as a program: re-executes itself with lavapipe, renders the box's front
//! at the size given (400 pixels if none), and writes what it found, one `key value` a line,
//! and on Linux its peak memory (`VmHWM`). Exits 1 if it can't render.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            let _ = writeln!(std::io::stderr(), "silico-render-probe: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    silico_render::lavapipe::reexec_with()?;
    let size = match std::env::args().nth(1) {
        Some(arg) => arg
            .parse::<u32>()
            .ok()
            .filter(|s| (1..=4096).contains(s))
            .ok_or_else(|| format!("{arg:?} isn't a size from 1 to 4096 pixels"))?,
        None => 400,
    };
    let adapter = silico_render::cpu_adapter()?;
    let probed = silico_render::probe::front_of_a_box(&adapter, size)?;
    let peak = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l["VmHWM:".len()..].trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let mut out = std::io::stdout().lock();
    let ids: Vec<String> = probed
        .ids
        .iter()
        .map(|((body, face), n)| format!("{body}:{face}={n}"))
        .collect();
    writeln!(out, "adapter {}", probed.adapter)
        .and_then(|()| writeln!(out, "size {size}"))
        .and_then(|()| writeln!(out, "ids {}", ids.join(",")))
        .and_then(|()| writeln!(out, "off {}", probed.off))
        .and_then(|()| writeln!(out, "context_seconds {:.3}", probed.context_seconds))
        .and_then(|()| writeln!(out, "render_seconds {:.3}", probed.render_seconds))
        .and_then(|()| writeln!(out, "peak_memory {peak}"))
        .map_err(|e| format!("couldn't write the result: {e}"))
}
