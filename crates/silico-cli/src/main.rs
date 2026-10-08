//! The `silico` command-line client.

use std::io::IsTerminal;
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

fn main() -> ExitCode {
    init_logging();
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "silico starting");
    ExitCode::SUCCESS
}

/// Logs go to stderr, so stdout stays free for command output. `SILICO_LOG` is a
/// `tracing` filter (default `warn`, also when empty); `SILICO_LOG_FORMAT` is `text` (the
/// default) or `json`, one JSON object per line, for agents and log tools. Colour is used
/// only when stderr is a terminal. A value that can't be used is reported and ignored.
fn init_logging() {
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

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());
    // try_init fails only if a subscriber is already set, which can't happen in main.
    let _ = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    for problem in problems {
        tracing::warn!("{problem}; using the default");
    }
}
