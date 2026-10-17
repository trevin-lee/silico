//! Builds PlaneGCS (third_party/planegcs, FreeCAD 1.1.3's, with its patch) and the cxx
//! bridge with Silico's deterministic recipe (target/occt/recipe.env, written by `pixi run
//! occt`; ARCHITECTURE.md, Deterministic kernel), against the pinned Eigen and Boost
//! headers from pixi, and links libc++ and the libm shim into every executable of this
//! crate, as silico-kernel does.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[path = "../silico-kernel/cxx_env.rs"]
mod cxx_env;
#[path = "../silico-kernel/recipe.rs"]
mod recipe;

use recipe::{BuildResult, check_cxx_env, read_recipe};

/// PlaneGCS's sources, under third_party/planegcs/src.
const PLANEGCS_SOURCES: &[&str] = &["GCS", "Constraints", "Geo", "SubSystem", "qp_eq"];

fn main() -> BuildResult<()> {
    check_cxx_env()?;
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or("silico-sketch isn't at crates/silico-sketch in the workspace")?;
    let recipe_path = root.join("target/occt/recipe.env");
    let recipe = read_recipe(&recipe_path).map_err(|e| {
        format!(
            "can't read {}: {e}. Run `pixi run occt` first: it writes the recipe every \
             worker's C++ is built with.",
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
    // The pinned toolchain's prefix (pixi's environment): libc++, and the Eigen and Boost
    // headers pixi.toml pins.
    let prefix = Path::new(cxx)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("can't find the toolchain's prefix from {cxx}"))?;
    let toolchain_lib = prefix.join("lib");
    let eigen = prefix.join("include/eigen3");
    for (what, path) in [
        ("Eigen", eigen.join("Eigen/Core")),
        (
            "Boost.Graph",
            prefix.join("include/boost/graph/connected_components.hpp"),
        ),
    ] {
        if !path.is_file() {
            return Err(format!(
                "{what} isn't in the pixi environment ({} is missing): run `pixi install`",
                path.display()
            )
            .into());
        }
    }
    let planegcs = root.join("third_party/planegcs");
    let src = planegcs.join("src");
    let stubs = planegcs.join("stubs");
    let infinity_nan = stubs.join("infinity-nan.h");

    let mut build = cxx_build::bridge("src/lib.rs");
    build
        .file("cpp/bridge.cpp")
        .compiler(cxx)
        .std("c++20")
        .cpp_link_stdlib("c++")
        // FreeCAD's release builds have NDEBUG: PlaneGCS's asserts are gone there too.
        .define("NDEBUG", None)
        // No SIMD, so Eigen sums the same way on every CPU; and the register count it
        // blocks products by fixed, rather than per architecture.
        .define("EIGEN_DONT_VECTORIZE", None)
        .define("EIGEN_ARCH_DEFAULT_NUMBER_OF_REGISTERS", Some("16"))
        // The stand-ins for FreeCAD's own headers, ahead of PlaneGCS's sources.
        .include(&stubs)
        .include(&src)
        .include(src.join("Mod/Sketcher/App"))
        .flag("-isystem")
        .flag(&eigen)
        // Boost from the environment's include directory, searched after the system's:
        // that directory holds other packages' headers too, which mustn't shadow libc's.
        .flag("-idirafter")
        .flag(prefix.join("include"))
        .flag("-include")
        .flag(&infinity_nan)
        // PlaneGCS's own warnings aren't Silico's to fix.
        .flag("-w");
    for name in PLANEGCS_SOURCES {
        build.file(src.join(format!("Mod/Sketcher/App/planegcs/{name}.cpp")));
    }
    for flag in get("SILICO_CXXFLAGS")?.split_whitespace() {
        build.flag(flag);
    }
    if env::var_os("CARGO_FEATURE_TEST_OPS").is_some() {
        build.define("SILICO_TEST_OPS", None);
    }
    build.try_compile("silico_sketch_bridge")?;

    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "cargo:rustc-link-search=native={}",
        toolchain_lib.display()
    )?;
    // Executables find libc++ at run time, and carry the shim.
    writeln!(
        out,
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        toolchain_lib.display()
    )?;
    writeln!(out, "cargo:rustc-link-arg={shim}")?;

    for path in ["src/lib.rs", "cpp/bridge.h", "cpp/bridge.cpp"] {
        writeln!(out, "cargo:rerun-if-changed={path}")?;
    }
    for path in [
        root.join("crates/silico-kernel/cxx_env.rs"),
        root.join("crates/silico-kernel/recipe.rs"),
        planegcs.clone(),
        recipe_path.clone(),
        PathBuf::from(shim),
    ] {
        writeln!(out, "cargo:rerun-if-changed={}", path.display())?;
    }
    // Anything the toolchain's headers come from (pixi.lock pins them).
    writeln!(
        out,
        "cargo:rerun-if-changed={}",
        root.join("pixi.lock").display()
    )?;
    let _ = fs::metadata(&planegcs)?;
    Ok(())
}
