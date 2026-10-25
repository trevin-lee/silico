//! M4.6's first step as a program: re-executes itself with lavapipe, renders the box's front
//! at the size given (400 pixels if none), and writes what it found, one `key value` a line,
//! and on Linux its peak and present memory (`VmHWM`, `VmRSS`). With `--hold SECONDS` after the
//! size, it then drops the render context and waits, writing its memory then, for the
//! footprint a worker keeps after a render (`ps -o rss` reads it on macOS). Exits 1 if it
//! can't render.

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
    let args: Vec<String> = std::env::args().collect();
    let hold = match args.iter().position(|a| a == "--hold") {
        Some(i) => Some(
            args.get(i + 1)
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("--hold takes a number of seconds")?,
        ),
        None => None,
    };
    let size = match args.get(1).filter(|a| *a != "--hold") {
        Some(arg) => arg
            .parse::<u32>()
            .ok()
            .filter(|s| (1..=4096).contains(s))
            .ok_or_else(|| format!("{arg:?} isn't a size from 1 to 4096 pixels"))?,
        None => 400,
    };
    let adapter = silico_render::cpu_adapter()?;
    let probed = silico_render::probe::front_of_a_box(&adapter, size)?;
    let peak = memory("VmHWM:");
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
        .and_then(|()| writeln!(out, "memory_after_render {}", memory("VmRSS:")))
        .and_then(|()| out.flush())
        .map_err(|e| format!("couldn't write the result: {e}"))?;
    if let Some(seconds) = hold {
        // The render context and its device were dropped with the probe's return.
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        writeln!(out, "memory_after_drop {}", memory("VmRSS:"))
            .and_then(|()| out.flush())
            .map_err(|e| format!("couldn't write the result: {e}"))?;
    }
    Ok(())
}

/// A line of `/proc/self/status` (Linux), or `unknown` where there's none.
fn memory(key: &str) -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_owned()))
        })
        .unwrap_or_else(|| "unknown".to_owned())
}
