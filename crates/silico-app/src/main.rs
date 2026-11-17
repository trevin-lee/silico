//! `silico-app`: the app's window. It starts with all four of its workers or not at all
//! (the rule `silico serve` keeps too), then warms one kernel worker and one sketch worker
//! off its UI thread (decision 10), starts reading its list of recent documents and its
//! panes' widths off it too, and opens a window drawn by `eframe` on wgpu, showing the start
//! page.

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use silico_app::App;
use silico_app::pane_widths::KeptLayout;
use silico_app::recent::Recents;
use silico_app::start::Warm;
use silico_app::theme;
use silico_app::thumbs::Thumbs;
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
                options(false),
                Box::new(move |creation| {
                    theme::apply(&creation.egui_ctx);
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
    // Warmed off the UI thread: a person's first rebuild needn't wait for a process. The
    // start page's status bar says how it stands.
    let warm = Warm::new();
    let warming = Arc::clone(&sessions);
    let told = warm.clone();
    let started = std::thread::Builder::new()
        .name("silico-warm".to_owned())
        .spawn(move || {
            let (kernel, sketches) = warming.warm(1, 1);
            tracing::info!(kernel, sketches, "warmed the workers");
            told.started(kernel, sketches);
        });
    if let Err(e) = started {
        tracing::warn!("the workers can't be warmed ahead of the first rebuild: {e}");
        warm.cold();
    }
    // Read off the UI thread, from the platform's app-data folder: the recent documents, their
    // thumbnails, and the side panes' widths beside them.
    let recents = Recents::default_file().map(Recents::at);
    let thumbs = Thumbs::default_folder().map(Thumbs::at);
    let widths = KeptLayout::default_file().map(KeptLayout::at);
    let silico = silico_beside();
    let ran = eframe::run_native(
        "Silico",
        app_options(silico_app::bar::TITLE_BAR_IS_APP_BAR),
        Box::new(move |creation| {
            theme::apply(&creation.egui_ctx);
            // Open and Browse… open the platform's own file dialogs, on this window.
            let app = App::new(sessions)
                .browsing(Box::new(silico_app::browse::Native::on(creation)))
                .warming(warm)
                .serving_with(silico);
            let app = match recents {
                Some(recents) => app.remembering(recents),
                None => app,
            };
            let app = match thumbs {
                Some(thumbs) => app.thumbnailing(thumbs),
                None => app,
            };
            let app = match widths {
                Some(widths) => app.keeping_layout(widths),
                None => app,
            };
            // The viewport drawn by silico-render on eframe's own device; with no wgpu, or
            // one it can't draw on, the app runs and its viewport says why it draws nothing.
            let app = match creation.wgpu_render_state.as_ref() {
                Some(state) => match silico_app::renderer::on_eframe(state) {
                    Ok(present) => app.presenting(Box::new(present)),
                    Err(why) => app.not_drawing(why),
                },
                None => app.not_drawing("the window has no wgpu device to draw with".to_owned()),
            };
            Ok(Box::new(app))
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

/// The `silico` beside this executable, its links resolved (where the workers are found,
/// and where a build puts it), for Copy MCP config; else `silico`, for the client's `PATH`.
fn silico_beside() -> String {
    silico_in(std::env::current_exe().and_then(std::fs::canonicalize).ok())
}

/// The `silico` in the folder of `exe`, if it's a file there; else `silico`.
fn silico_in(exe: Option<std::path::PathBuf>) -> String {
    let name = format!("silico{}", std::env::consts::EXE_SUFFIX);
    exe.and_then(|exe| exe.parent().map(|folder| folder.join(&name)))
        .filter(|silico| silico.is_file())
        .and_then(|silico| silico.into_os_string().into_string().ok())
        .unwrap_or_else(|| "silico".to_owned())
}

/// The window's options: drawn by eframe on wgpu; with `title_bar_is_app_bar` (the app's
/// window, on macOS), its content under a title bar with no title, drawn through, so the
/// app bar is the title bar (as Rerun's is). The window for missing workers, which has no
/// app bar, keeps its title bar.
fn options(title_bar_is_app_bar: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Silico")
            .with_inner_size([1280.0, 800.0])
            .with_fullsize_content_view(title_bar_is_app_bar)
            .with_title_shown(!title_bar_is_app_bar)
            .with_titlebar_shown(!title_bar_is_app_bar),
        ..Default::default()
    }
}

/// The app's window's options: [`options`], no smaller than the panes, the bars, and a
/// usable canvas need ([`theme::WINDOW_MIN_WIDTH`], [`theme::WINDOW_MIN_HEIGHT`]), as
/// Fusion's window keeps a least size.
fn app_options(title_bar_is_app_bar: bool) -> eframe::NativeOptions {
    let mut options = options(title_bar_is_app_bar);
    options.viewport = options
        .viewport
        .with_min_inner_size([theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT]);
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_mcp_configs_command_is_the_silico_beside_the_app_else_silico() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir
            .path()
            .join(format!("silico-app{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&app, b"").unwrap();
        // Nothing beside it, a folder of that name, or no executable known: the client's PATH's.
        assert_eq!(silico_in(Some(app.clone())), "silico");
        assert_eq!(silico_in(None), "silico");
        let silico = dir
            .path()
            .join(format!("silico{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir(&silico).unwrap();
        assert_eq!(silico_in(Some(app.clone())), "silico");
        std::fs::remove_dir(&silico).unwrap();
        // A file beside it: that file, by its path.
        std::fs::write(&silico, b"").unwrap();
        assert_eq!(silico_in(Some(app)).as_str(), silico.to_str().unwrap());
        // This test's own executable, in cargo's deps folder, has none beside it (cargo names
        // each binary there with a hash).
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert!(!exe.with_file_name(silico.file_name().unwrap()).exists());
        assert_eq!(silico_beside(), "silico");
    }

    #[test]
    fn the_title_bar_is_drawn_through_only_for_the_app_bar() {
        // With the app bar as the title bar: the content under it, no title and no title
        // bar of the platform's (as Rerun's window); without, the platform's own.
        for (app_bar, platforms) in [(true, false), (false, true)] {
            let viewport = options(app_bar).viewport;
            assert_eq!(viewport.fullsize_content_view, Some(app_bar), "{app_bar}");
            assert_eq!(viewport.title_shown, Some(platforms), "{app_bar}");
            assert_eq!(viewport.titlebar_shown, Some(platforms), "{app_bar}");
        }
    }

    #[test]
    fn the_apps_window_is_no_smaller_than_its_panes_bars_and_a_usable_canvas() {
        // By hand from styles.css and the theme's proposals. Wide: the wider of the toolbar
        // in sketch mode, 881.5 in the app's fonts, on the 8-point grid 888, and the side
        // panes at their leasts (pane_widths.rs: the browser's 200, the property panel's 232)
        // with a canvas that holds a banner at its least, 320, 10 at each side. High:
        // `--top-bar-h` 30; the toolbar, `--toolbar-tabs-h` 24, `--toolbar-h` 52, and its 1
        // border; a canvas that holds the ViewCube, 16 from its top and 92 high, over the
        // navigation bar, 32 high and 12 above and below it; egui's 1 line over the
        // timeline; `--timeline-h` 40; and `--status-h` 24.
        let panes: f32 = 200.0 + 232.0 + (10.0 + 320.0 + 10.0);
        assert_eq!(panes, 772.0);
        let wide = panes.max(888.0);
        let high =
            30.0 + (24.0 + 52.0 + 1.0) + (16.0 + 92.0 + 12.0 + 32.0 + 12.0) + 1.0 + 40.0 + 24.0;
        assert_eq!((wide, high), (888.0, 336.0));
        for app_bar in [true, false] {
            let viewport = app_options(app_bar).viewport;
            assert_eq!(
                viewport.min_inner_size,
                Some(eframe::egui::vec2(wide, high)),
                "{app_bar}"
            );
            // The rest as `options` gives it.
            assert_eq!(viewport.fullsize_content_view, Some(app_bar), "{app_bar}");
            assert_eq!(viewport.inner_size, options(app_bar).viewport.inner_size);
        }
    }
}
