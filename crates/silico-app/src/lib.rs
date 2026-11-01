//! Silico's app (docs/design/m5.0-app.md; M5.4, the app's shell): a window that is one
//! client of the core's command API, as MCP's server and the CLI are (invariant 3), drawing
//! what its commands answer and never waiting on one (invariant 4). Laid out as Fusion's
//! window is, with Rerun's look as the UI prototype tokens it (`theme`), Inter for its
//! text, and its own icons (`icons`). With no document open it shows the start page
//! (`start`), which is also its Home tab, and the documents opened lately (`recent`).

pub mod app;
pub mod browse;
pub mod dialog;
pub mod export;
pub mod icons;
pub mod missing;
pub mod navigate;
mod panes;
pub mod recent;
pub mod renderer;
pub mod start;
pub mod tab;
pub mod theme;
pub mod viewport;

pub use app::App;
