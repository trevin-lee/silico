//! Builds the cxx bridge with Silico's deterministic recipe (target/occt/recipe.env,
//! written by `pixi run occt`) and links OCCT, libc++, and the libm shim.
//!
//! The shim is linked into every executable of this crate (the worker, test binaries,
//! benches, examples) as a link argument, so the bridge's math and Rust's own come from
//! it, never from the platform's math library (ARCHITECTURE.md, Deterministic kernel).

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[path = "cxx_env.rs"]
mod cxx_env;

type BuildResult<T> = Result<T, Box<dyn Error>>;

/// The OCCT libraries the bridge calls into directly. Their own dependencies are found
/// through the libraries themselves.
const OCCT_LIBS: &[&str] = &[
    "TKPrim",
    "TKTopAlgo",
    "TKBRep",
    "TKGeomBase",
    "TKG3d",
    "TKG2d",
    "TKMath",
    "TKernel",
];

fn main() -> BuildResult<()> {
    check_cxx_env()?;
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or("silico-kernel isn't at crates/silico-kernel in the workspace")?;
    let occt = root.join("target/occt");
    let recipe_path = occt.join("recipe.env");
    let recipe = read_recipe(&recipe_path).map_err(|e| {
        format!(
            "can't read {}: {e}. Run `pixi run occt` first; the recipe exists only while \
             OCCT's checks pass.",
            recipe_path.display()
        )
    })?;
    let get = |key: &str| -> BuildResult<&str> {
        recipe
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("{} has no {key}", recipe_path.display()).into())
    };
    let cxx = get("SILICO_CXX")?;
    let shim = get("SILICO_LIBM_SHIM")?;
    // Through the checkout's own link into the build cache, so a rebuilt entry is picked up.
    let occt_lib = occt.join("install/lib");
    let occt_include = occt.join("install/include/opencascade");
    // libc++ comes from the pinned toolchain, next to its compiler.
    let toolchain_lib = Path::new(cxx)
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("lib"))
        .ok_or_else(|| format!("can't find the toolchain's lib directory from {cxx}"))?;

    // C++17 is OCCT 8's own minimum, so the bridge can include any OCCT header.
    let mut build = cxx_build::bridge("src/lib.rs");
    build
        .file("cpp/bridge.cpp")
        .compiler(cxx)
        .std("c++17")
        .cpp_link_stdlib("c++")
        .include(&occt_include);
    for flag in get("SILICO_CXXFLAGS")?.split_whitespace() {
        build.flag(flag);
    }
    if env::var_os("CARGO_FEATURE_TEST_OPS").is_some() {
        build.define("SILICO_TEST_OPS", None);
    }
    build.try_compile("silico_kernel_bridge")?;

    let mut out = std::io::stdout().lock();
    writeln!(out, "cargo:rustc-link-search=native={}", occt_lib.display())?;
    writeln!(
        out,
        "cargo:rustc-link-search=native={}",
        toolchain_lib.display()
    )?;
    for lib in OCCT_LIBS {
        writeln!(out, "cargo:rustc-link-lib=dylib={lib}")?;
    }
    // Executables find OCCT and libc++ at run time, and carry the shim.
    writeln!(
        out,
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        occt_lib.display()
    )?;
    writeln!(
        out,
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        toolchain_lib.display()
    )?;
    writeln!(out, "cargo:rustc-link-arg={shim}")?;

    for path in ["src/lib.rs", "cpp/bridge.h", "cpp/bridge.cpp", "cxx_env.rs"] {
        writeln!(out, "cargo:rerun-if-changed={path}")?;
    }
    writeln!(out, "cargo:rerun-if-changed={}", recipe_path.display())?;
    writeln!(out, "cargo:rerun-if-changed={shim}")?;
    Ok(())
}

/// Refuses C++ compiler variables that would change how Silico's C++ is compiled
/// (cxx_env.rs). The C++ settings themselves are forced in .cargo/config.toml, not here:
/// the cxx crate's own runtime is compiled by its build script, before this one runs.
fn check_cxx_env() -> BuildResult<()> {
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
fn read_recipe(path: &Path) -> BuildResult<BTreeMap<String, String>> {
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
