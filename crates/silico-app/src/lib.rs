//! Silico's app (docs/design/m5.0-app.md; M5.4, the app's shell): a window that is one
//! client of the core's command API, as MCP's server and the CLI are (invariant 3), drawing
//! what its commands answer and never waiting on one (invariant 4). Laid out as Fusion's
//! window is, with Rerun's look as the UI prototype tokens it (`theme`), Inter for its
//! text, and its own icons (`icons`), under its application bar (`bar`), which on macOS is
//! the window's title bar. With no document open it shows the start page (`start`), which is
//! also its Home tab, and the documents opened lately (`recent`).

pub mod app;
mod app_data;
pub mod arrow;
mod banner;
pub mod bar;
pub mod box_select;
pub mod boxes;
pub mod browse;
mod canvas_hint;
pub mod canvas_menu;
pub mod changes;
pub mod clicks;
mod columns;
pub mod dialog;
pub mod export;
pub mod feature_menu;
pub mod grid;
pub mod icons;
pub mod measures;
mod menu;
pub mod missing;
mod modal;
pub mod navigate;
pub mod pane_widths;
mod panes;
pub mod planes;
mod properties;
pub mod recent;
pub mod renderer;
mod repair;
pub mod select_other;
pub mod selection;
mod sheet;
pub mod shortcuts;
pub mod sketch;
pub mod start;
pub mod status;
pub mod tab;
pub mod theme;
pub mod timeline;
pub mod toast;
pub mod toolbar;
pub mod triad;
pub mod viewport;
mod wake;
mod whats_wrong;
mod words;

pub use app::App;
