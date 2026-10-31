//! `silico-app`: the app's window. It starts with all four of its workers or not at all
//! (the rule `silico serve` keeps too), then warms one kernel worker and one sketch worker
//! off its UI thread (decision 10), and opens a window drawn by `eframe` on wgpu.

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use silico_app::App;
use silico_core::command::Sessions;

fn main() -> ExitCode {
    silico_core::logging::init();
    let workers = match silico_core::workers::find() {
        Ok(workers) => workers,
        Err(missing) => {
            {
                let mut stderr = std::io::stderr().lock();
                for why in &missing {
                    let _ = writeln!(stderr, "silico-app: {why}");
                }
            }
            // Shown in a window too, which needs no worker, for a launch with no terminal.
            let shown = eframe::run_native(
                "Silico",
                options(),
                Box::new(move |creation| {
                    silico_app::theme::apply(&creation.egui_ctx);
                    Ok(Box::new(silico_app::missing::Missing::new(missing)))
                }),
            );
            if let Err(e) = shown {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "silico-app: the window failed: {e}"
                );
            }
            return ExitCode::from(2);
        }
    };
    let sessions = Arc::new(Sessions::new(workers.kernel).rendering(workers.renderer));
    // Warmed off the UI thread: a person's first rebuild needn't wait for a process.
    let warming = Arc::clone(&sessions);
    let warm = std::thread::Builder::new()
        .name("silico-warm".to_owned())
        .spawn(move || {
            let (kernel, sketches) = warming.warm(1, 1);
            tracing::info!(kernel, sketches, "warmed the workers");
        });
    if let Err(e) = warm {
        tracing::warn!("the workers can't be warmed ahead of the first rebuild: {e}");
    }
    let ran = eframe::run_native(
        "Silico",
        options(),
        Box::new(move |creation| {
            silico_app::theme::apply(&creation.egui_ctx);
            Ok(Box::new(App::new(sessions)))
        }),
    );
    match ran {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "silico-app: the window failed: {e}"
            );
            ExitCode::from(1)
        }
    }
}

/// The window's options: drawn by eframe on wgpu.
fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Silico")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    }
}
