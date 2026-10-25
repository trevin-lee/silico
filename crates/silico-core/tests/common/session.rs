//! For tests of sessions (`silico_core::command::Sessions`): waiting for a command's answer,
//! failing if it hangs, and reading the one error a refusal gives.

use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::command::{Response, Running};
use silico_core::document::FeatureId;

/// Longer than any command here takes on a loaded machine: past it, a command hangs.
pub const HANG: Duration = Duration::from_secs(60);

/// The command's answer, failing if it hangs.
pub fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

/// The one error a refused response gives: its code, message, parameter, and suggestion.
pub fn refusal<T: std::fmt::Debug>(
    response: &Response<T>,
) -> (ErrorCode, String, Option<String>, Option<String>) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let error = &problems.errors()[0];
    (
        error.code(),
        error.message().to_owned(),
        error.parameter().map(str::to_owned),
        error.suggestion().map(str::to_owned),
    )
}

pub fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}
