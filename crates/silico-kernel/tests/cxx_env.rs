//! The rule build.rs applies to C++ compiler variables (cxx_env.rs): only the values
//! .cargo/config.toml forces may be set.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../cxx_env.rs"]
mod cxx_env;

#[path = "common/imports.rs"]
mod imports;

fn rejected(vars: &[(&str, &str)]) -> Vec<String> {
    cxx_env::rejected_cxx_vars(vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())))
}

#[test]
fn forced_values_and_unrelated_variables_are_accepted() {
    let ok = [
        ("CXX", "/any/checkout/.pixi/envs/default/bin/clang++"),
        ("CXXFLAGS", "-stdlib=libc++"),
        ("CXXSTDLIB", "c++"),
        ("CC", "gcc"),
        ("CFLAGS", "-O3"),
        ("CXXBRIDGE_DEBUG", "1"),
        ("RUSTFLAGS", "-Ctarget-cpu=native"),
    ];
    assert_eq!(rejected(&ok), Vec::<String>::new());
}

#[test]
fn every_other_cxx_variable_is_rejected() {
    let bad = [
        ("CXXFLAGS", "-ffp-contract=fast"),
        ("CXXFLAGS", "-stdlib=libc++ -ffast-math"),
        ("CXXFLAGS_aarch64-apple-darwin", "-ffp-contract=fast"),
        ("CXXFLAGS_x86_64_unknown_linux_gnu", "-ffast-math"),
        ("TARGET_CXXFLAGS", "-march=native"),
        ("HOST_CXXFLAGS", "-O3"),
        ("CXX_x86_64-unknown-linux-gnu", "g++"),
        ("HOST_CXX", "g++"),
        ("TARGET_CXX", "g++"),
        ("CXXSTDLIB", "stdc++"),
        ("CXXSTDLIB_aarch64-apple-darwin", "c++"),
        ("TARGET_CXXSTDLIB", "stdc++"),
    ];
    for (key, value) in bad {
        assert_eq!(
            rejected(&[(key, value)]),
            [format!("{key}={value:?}")],
            "{key}={value} was accepted"
        );
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
