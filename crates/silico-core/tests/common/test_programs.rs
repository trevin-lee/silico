//! For a crate whose tests are built as a few programs (Cargo.toml's `autotests = false`):
//! that every test file is built, a `[[test]]` of its own or a module of one. Without this
//! check, a new file nobody adds would never be built or run, and nothing would say so.
//! Shared by those crates' tests through `#[path]`.

use std::collections::BTreeSet;
use std::path::Path;

/// Fails naming each `tests/*.rs` file of the crate at `manifest_dir` that no test program
/// builds: neither a `[[test]]`'s `path` in its Cargo.toml nor a `#[path]` module of one.
pub fn assert_every_test_file_is_built(manifest_dir: &str) {
    let dir = Path::new(manifest_dir);
    let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let programs: Vec<String> = cargo
        .lines()
        .filter_map(|l| l.trim().strip_prefix("path = \"tests/"))
        .filter_map(|rest| rest.strip_suffix(".rs\""))
        .map(str::to_owned)
        .collect();
    let mut built: BTreeSet<String> = programs.iter().cloned().collect();
    for program in &programs {
        let text = std::fs::read_to_string(dir.join(format!("tests/{program}.rs"))).unwrap();
        for line in text.lines() {
            if let Some(name) = line
                .trim()
                .strip_prefix("#[path = \"")
                .and_then(|rest| rest.strip_suffix(".rs\"]"))
                .filter(|name| !name.contains('/'))
            {
                built.insert(name.to_owned());
            }
        }
    }
    let files: BTreeSet<String> = std::fs::read_dir(dir.join("tests"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    let unbuilt: Vec<&String> = files.difference(&built).collect();
    assert!(
        unbuilt.is_empty(),
        "test files no test program builds: {unbuilt:?}. Add each as a `#[path]` module of a \
         group program (tests/all_*.rs) or as a [[test]] in Cargo.toml"
    );
}
