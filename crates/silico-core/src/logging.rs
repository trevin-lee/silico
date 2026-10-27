//! Logging for the app's processes (the CLI, the debug viewer, later the app and the MCP
//! server). Workers log differently: JSON lines on stderr, which the core re-emits
//! (`silico_kernel::worker::run`).

use std::io::IsTerminal;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

/// Logs go to stderr, so stdout stays free for command output. `SILICO_LOG` is a
/// `tracing` filter (default `warn`, also when empty); `SILICO_LOG_FORMAT` is `text` (the
/// default) or `json`, one JSON object per line, for agents and log tools. Colour is used
/// only when stderr is a terminal. A value that can't be used is reported and ignored.
///
/// Each session's events are kept for `read_logs` too, at every level, whatever
/// `SILICO_LOG` lets through to stderr ([`crate::command::logs::layer`]): the filter is the
/// stderr log's own.
pub fn init() {
    let mut problems = Vec::new();

    let default_filter = || EnvFilter::new(LevelFilter::WARN.to_string());
    let filter = match std::env::var_os("SILICO_LOG") {
        None => default_filter(),
        Some(raw) => match raw.to_str() {
            None => {
                problems.push(format!("ignoring SILICO_LOG={raw:?}: not UTF-8"));
                default_filter()
            }
            Some("") => default_filter(),
            Some(value) => EnvFilter::try_new(value).unwrap_or_else(|e| {
                problems.push(format!("ignoring SILICO_LOG={value:?}: {e}"));
                default_filter()
            }),
        },
    };

    let json = match std::env::var_os("SILICO_LOG_FORMAT") {
        None => false,
        Some(raw) => match raw.to_str() {
            Some("json") => true,
            Some("" | "text") => false,
            _ => {
                problems.push(format!(
                    "ignoring SILICO_LOG_FORMAT={raw:?}: expected json or text"
                ));
                false
            }
        },
    };

    let stderr = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());
    let stderr = if json {
        stderr.json().with_filter(filter).boxed()
    } else {
        stderr.with_filter(filter).boxed()
    };
    // Only the first call in a process takes effect: try_init fails, and does nothing, if a
    // subscriber is already set. Call this once, at the start of main.
    let _ = tracing_subscriber::registry()
        .with(crate::command::logs::layer())
        .with(stderr)
        .try_init();
    for problem in problems {
        tracing::warn!("{problem}; using the default");
    }
}
