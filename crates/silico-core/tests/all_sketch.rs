//! silico-core's sketches' tests (their format, gate, solving, features, and stored
//! coordinates), as one test program: each file a module, its tests as they were, the
//! modules they share loaded once: fewer programs to build and link than one a file, for
//! every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/command.rs"]
mod commands;
#[path = "common/sketch.rs"]
mod fixture;
#[path = "common/stand_in.rs"]
mod stand_in;
#[path = "common/sweeps.rs"]
mod sweeps;
#[path = "common/within.rs"]
mod within;

#[path = "adversarial_decision5_parts.rs"]
mod adversarial_decision5_parts;
#[path = "adversarial_load_path.rs"]
mod adversarial_load_path;
#[path = "adversarial_redundant_resolve.rs"]
mod adversarial_redundant_resolve;
#[path = "adversarial_sketch.rs"]
mod adversarial_sketch;
#[path = "adversarial_sketch_edit.rs"]
mod adversarial_sketch_edit;
#[path = "adversarial_sketch_features.rs"]
mod adversarial_sketch_features;
#[path = "adversarial_sketch_solve.rs"]
mod adversarial_sketch_solve;
#[path = "adversarial_sketch_split.rs"]
mod adversarial_sketch_split;
#[path = "adversarial_stored_coordinates.rs"]
mod adversarial_stored_coordinates;
#[path = "sketch.rs"]
mod sketch;
#[path = "sketch_edit.rs"]
mod sketch_edit;
#[path = "sketch_features.rs"]
mod sketch_features;
#[path = "sketch_features_gate.rs"]
mod sketch_features_gate;
#[path = "sketch_gate.rs"]
mod sketch_gate;
#[path = "sketch_solve.rs"]
mod sketch_solve;
#[path = "sketch_split.rs"]
mod sketch_split;
#[path = "stored_coordinates.rs"]
mod stored_coordinates;
