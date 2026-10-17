//! What every crate that builds worker C++ reads at build time (included by each one's
//! build.rs with `#[path]`, after `cxx_env`): Silico's deterministic recipe,
//! `target/occt/recipe.env`, which `pixi run occt` writes, and the rule for C++ compiler
//! variables (cxx_env.rs).

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::cxx_env;

pub type BuildResult<T> = Result<T, Box<dyn Error>>;

/// Refuses C++ compiler variables that would change how Silico's C++ is compiled
/// (cxx_env.rs). The C++ settings themselves are forced in .cargo/config.toml, not here:
/// the cxx crate's own runtime is compiled by its build script, before this one runs.
pub fn check_cxx_env() -> BuildResult<()> {
    let mut out = std::io::stdout().lock();
    for (key, _) in env::vars() {
        if cxx_env::is_cxx_var(&key) {
            writeln!(out, "cargo:rerun-if-env-changed={key}")?;
        }
    }
    for name in cxx_env::CXX_VARS {
        for prefix in ["", "HOST_", "TARGET_"] {
            writeln!(out, "cargo:rerun-if-env-changed={prefix}{name}")?;
        }
    }
    let rejected = cxx_env::rejected_cxx_vars(env::vars());
    if !rejected.is_empty() {
        return Err(format!(
            "{} would change how Silico's C++ is compiled; unset it (.cargo/config.toml sets \
             the C++ compiler and flags)",
            rejected.join(", ")
        )
        .into());
    }
    Ok(())
}

/// Reads `KEY="value"` lines, skipping comments and blank lines.
pub fn read_recipe(path: &Path) -> BuildResult<BTreeMap<String, String>> {
    let mut recipe = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("not KEY=\"value\": {line}"))?;
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("value isn't quoted: {line}"))?;
        recipe.insert(key.to_owned(), value.to_owned());
    }
    Ok(recipe)
}
