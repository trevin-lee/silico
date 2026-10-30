//! Silico's app (docs/design/m5.0-app.md; M5.4, the app's shell): a window that is one
//! client of the core's command API, as MCP's server and the CLI are (invariant 3), drawing
//! what its commands answer and never waiting on one (invariant 4). Laid out as Fusion's
//! window is, with Rerun's look (`theme`), and Inter as its one font.

pub mod app;
mod panes;
pub mod tab;
pub mod theme;

pub use app::App;
