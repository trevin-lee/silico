//! The render bench (CI's `render` bench): restarts itself with lavapipe as the worker does,
//! draws `silico_render::bench::front_of_a_box` at the size given (400 pixels square if
//! none) through the worker's renderer, and writes what it took, one `key value` a line: the
//! adapter, the time to make the render context and to render, the PNG's bytes, and on
//! Linux its peak memory and its memory with the context held (`VmHWM`, `VmRSS`). With
//! `--hold SECONDS` after the size, it then drops the context and waits, and writes its
//! memory then, for the footprint a worker keeps after a render (`ps -o rss` reads it on
//! macOS). Exits 1, saying why, if its arguments are wrong or it can't render.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use silico_render::render::Renderer;
use silico_render::worker::RenderAdapter;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            let _ = writeln!(std::io::stderr(), "silico-render-bench: {problem}");
            ExitCode::FAILURE
        }
    }
}

/// The size and the seconds to hold, from the arguments after the program's name.
fn arguments(args: &[OsString]) -> Result<(u32, Option<u64>), String> {
    let text = |a: &OsString| {
        a.to_str()
            .map(str::to_owned)
            .ok_or(format!("{a:?} isn't text"))
    };
    let (size, rest) = match args.first() {
        Some(first) if first != "--hold" => (Some(text(first)?), &args[1..]),
        _ => (None, args),
    };
    let size = match size {
        Some(arg) => arg
            .parse::<u32>()
            .ok()
            .filter(|s| (1..=silico_protocol::MAX_RENDER_SIDE).contains(s))
            .ok_or_else(|| format!("{arg:?} isn't a size from 1 to 4096 pixels"))?,
        None => 400,
    };
    let hold = match rest {
        [] => None,
        [flag, seconds] if flag == "--hold" => Some(
            text(seconds)?
                .parse::<u64>()
                .map_err(|_| "--hold takes a number of seconds".to_owned())?,
        ),
        [flag] if flag == "--hold" => return Err("--hold takes a number of seconds".to_owned()),
        _ => return Err(format!("unexpected arguments: {rest:?}")),
    };
    Ok((size, hold))
}

fn run() -> Result<(), String> {
    silico_render::lavapipe::reexec_with()?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (size, hold) = arguments(&args)?;
    let adapter = silico_render::adapter(RenderAdapter::Software)?;
    let started = Instant::now();
    let mut renderer = Renderer::new(&adapter)?;
    let context = started.elapsed();
    let rendering = Instant::now();
    let rendered = renderer.render(&silico_render::bench::front_of_a_box(size, size))?;
    let render = rendering.elapsed();
    let held = memory("VmRSS:");
    let peak = memory("VmHWM:");
    let mut out = std::io::stdout().lock();
    writeln!(out, "adapter {}", adapter.get_info().name)
        .and_then(|()| writeln!(out, "size {size}"))
        .and_then(|()| writeln!(out, "png_bytes {}", rendered.png.len()))
        .and_then(|()| writeln!(out, "context_seconds {:.3}", context.as_secs_f64()))
        .and_then(|()| writeln!(out, "render_seconds {:.3}", render.as_secs_f64()))
        .and_then(|()| writeln!(out, "peak_memory {peak}"))
        .and_then(|()| writeln!(out, "memory_with_context {held}"))
        .and_then(|()| out.flush())
        .map_err(|e| format!("couldn't write the result: {e}"))?;
    if let Some(seconds) = hold {
        drop(renderer);
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
