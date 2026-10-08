//! Which C++ compiler variables the build may see. Shared by build.rs, which refuses to
//! build with any other, and tests/cxx_env.rs, which proves the rule (through `#[path]`).
//!
//! The cc crate appends C++ flags from the environment after the recipe's, so they would
//! override it (a shell's `CXXFLAGS=-ffp-contract=fast` would cancel the recipe's
//! `-ffp-contract=off`), and a per-target compiler or standard library would mix C++
//! runtimes. .cargo/config.toml forces CXX, CXXFLAGS, and CXXSTDLIB; any other variable cc
//! reads for C++ (with a `_<target>` suffix or a `HOST_`/`TARGET_` prefix) is refused.

/// The C++ variables cc reads, without prefix or suffix.
pub const CXX_VARS: &[&str] = &["CXX", "CXXFLAGS", "CXXSTDLIB"];

/// The values .cargo/config.toml forces. CXX is forced too, to a path that varies by
/// checkout, so any value of plain CXX is accepted.
const FORCED: &[(&str, &str)] = &[("CXXFLAGS", "-stdlib=libc++"), ("CXXSTDLIB", "c++")];

/// Whether cc would read `key` when compiling C++.
pub fn is_cxx_var(key: &str) -> bool {
    let base = key
        .strip_prefix("HOST_")
        .or_else(|| key.strip_prefix("TARGET_"))
        .unwrap_or(key);
    CXX_VARS
        .iter()
        .any(|name| base == *name || base.starts_with(&format!("{name}_")))
}

/// The variables among `vars` that would change how Silico's C++ is compiled, as
/// `KEY="value"`.
pub fn rejected_cxx_vars<I>(vars: I) -> Vec<String>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(key, value)| {
            is_cxx_var(key)
                && key != "CXX"
                && !FORCED
                    .iter()
                    .any(|(name, forced)| key == name && value == forced)
        })
        .map(|(key, value)| format!("{key}={value:?}"))
        .collect()
}
