//! silico-kernel's naming tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/close.rs"]
mod close;
#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/oracle.rs"]
mod oracle;
#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

#[path = "adversarial_naming.rs"]
mod adversarial_naming;
#[path = "adversarial_references.rs"]
mod adversarial_references;
#[path = "adversarial_sweep_faces.rs"]
mod adversarial_sweep_faces;
#[path = "adversarial_type_words.rs"]
mod adversarial_type_words;
#[path = "history.rs"]
mod history;
#[path = "history_fixtures.rs"]
mod history_fixtures;
#[path = "name_collisions.rs"]
mod name_collisions;
#[path = "naming.rs"]
mod naming;
#[path = "references.rs"]
mod references;
#[path = "sketch_feature_names.rs"]
mod sketch_feature_names;
#[path = "sweep_faces.rs"]
mod sweep_faces;
#[path = "topology.rs"]
mod topology;
