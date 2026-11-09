//! Silico's app (docs/design/m5.0-app.md; M5.4, the app's shell): a window that is one
//! client of the core's command API, as MCP's server and the CLI are (invariant 3), drawing
//! what its commands answer and never waiting on one (invariant 4). Laid out as Fusion's
//! window is, with Rerun's look as the UI prototype tokens it (`theme`), Inter for its
//! text, and its own icons (`icons`), under its application bar (`bar`), which on macOS is
//! the window's title bar. With no document open it shows the start page (`start`), which is
//! also its Home tab, and the documents opened lately (`recent`).

pub mod app;
mod banner;
pub mod bar;
pub mod box_select;
pub mod browse;
pub mod changes;
pub mod dialog;
pub mod export;
pub mod feature_menu;
pub mod icons;
pub mod missing;
mod modal;
pub mod navigate;
mod panes;
mod properties;
pub mod recent;
pub mod renderer;
mod repair;
pub mod selection;
mod sheet;
pub mod shortcuts;
pub mod sketch;
pub mod start;
pub mod status;
pub mod tab;
pub mod theme;
pub mod timeline;
pub mod toolbar;
pub mod viewport;
mod wake;
mod whats_wrong;

pub use app::App;
