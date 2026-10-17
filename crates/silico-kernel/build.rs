//! Builds the cxx bridge with Silico's deterministic recipe (target/occt/recipe.env,
//! written by `pixi run occt`) and links OCCT, libc++, and the libm shim.
//!
//! The shim is linked into every executable of this crate (the worker, test binaries,
//! benches, examples) as a link argument, so the bridge's math and Rust's own come from
//! it, never from the platform's math library (ARCHITECTURE.md, Deterministic kernel).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[path = "cxx_env.rs"]
mod cxx_env;
#[path = "recipe.rs"]
mod recipe;

use recipe::{BuildResult, check_cxx_env, read_recipe};

/// The OCCT libraries the bridge calls into directly. Their own dependencies are found
/// through the libraries themselves.
const OCCT_LIBS: &[&str] = &[
    "TKFillet",
    "TKBO",
    "TKBool",
    "TKMesh",
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
    // The build cache's entry the checkout's link points at (`<cache>/<hash of OCCT's
    // inputs>/install`), not the link itself: the worker loads OCCT from where it was
    // linked, so pointing the checkout at another build (`pixi run occt`, here or in another
    // checkout sharing target/occt) can't change the OCCT a built worker runs on. That
    // entry's name is compiled into the worker too: the core's cache keys results by the
    // worker's bytes, and OCCT is linked dynamically, so its own bytes aren't the worker's.
    let link = occt.join("install");
    let install =
        fs::read_link(&link).map_err(|e| format!("can't read the link {}: {e}", link.display()))?;
    let occt_lib = install.join("lib");
    let occt_include = install.join("include/opencascade");
    // libc++ comes from the pinned toolchain, next to its compiler.
    let toolchain_lib = Path::new(cxx)
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("lib"))
        .ok_or_else(|| format!("can't find the toolchain's lib directory from {cxx}"))?;

    // C++17 builds every OCCT header. OCCT's headers are system headers to the bridge, so
    // their own warnings (OCCT 7.9's use of sprintf) don't drown out the bridge's.
    let mut build = cxx_build::bridge("src/lib.rs");
    build
        .file("cpp/bridge.cpp")
        .compiler(cxx)
        .std("c++17")
        .cpp_link_stdlib("c++")
        .flag("-isystem")
        .flag(&occt_include);
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

    for path in [
        "src/lib.rs",
        "cpp/bridge.h",
        "cpp/bridge.cpp",
        "cxx_env.rs",
        "recipe.rs",
    ] {
        writeln!(out, "cargo:rerun-if-changed={path}")?;
    }
    writeln!(out, "cargo:rerun-if-changed={}", recipe_path.display())?;
    writeln!(out, "cargo:rerun-if-changed={shim}")?;
    writeln!(
        out,
        "cargo:rustc-env=SILICO_OCCT_BUILD={}",
        install.display()
    )?;
    writeln!(out, "cargo:rerun-if-changed={}", link.display())?;
    Ok(())
}
