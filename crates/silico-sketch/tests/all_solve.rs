//! silico-sketch's solve tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/test_programs.rs"]
mod test_programs;

// Shared by the files below, each loaded once.
#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/generate.rs"]
mod generate;
#[path = "common/saves.rs"]
mod saves;
#[path = "common/systems.rs"]
mod systems;

#[path = "adversarial_decision5_bridge.rs"]
mod adversarial_decision5_bridge;
#[path = "adversarial_decision5_extent.rs"]
mod adversarial_decision5_extent;
#[path = "adversarial_decision5_parts.rs"]
mod adversarial_decision5_parts;
#[path = "adversarial_planegcs.rs"]
mod adversarial_planegcs;
#[path = "adversarial_redundant_resolve.rs"]
mod adversarial_redundant_resolve;
#[path = "fixed_point.rs"]
mod fixed_point;
#[path = "forms.rs"]
mod forms;
#[path = "solve.rs"]
mod solve;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
