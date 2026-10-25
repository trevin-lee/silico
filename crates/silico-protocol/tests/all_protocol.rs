//! silico-protocol's integration tests, all but allocation's, as one test program: each
//! file a module, its tests as they were: fewer programs to build and link than one a file,
//! for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/test_programs.rs"]
mod test_programs;

#[path = "adversarial_describe.rs"]
mod adversarial_describe;
#[path = "adversarial_parts.rs"]
mod adversarial_parts;
#[path = "adversarial_profile.rs"]
mod adversarial_profile;
#[path = "adversarial_sweep.rs"]
mod adversarial_sweep;
#[path = "frames.rs"]
mod frames;
#[path = "history.rs"]
mod history;
#[path = "primitive.rs"]
mod primitive;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
