//! silico-render's integration tests, as one test program: each file a module, its tests
//! as they were: fewer programs to build and link, each linking wgpu and re_renderer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/test_programs.rs"]
mod test_programs;

#[path = "adversarial_render.rs"]
mod adversarial_render;
#[path = "bench.rs"]
mod bench;
#[path = "worker.rs"]
mod worker;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
