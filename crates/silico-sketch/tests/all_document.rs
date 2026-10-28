//! silico-sketch's document tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/generate.rs"]
mod generate;
#[path = "../../silico-core/tests/common/lateness.rs"]
mod lateness;
#[path = "common/saves.rs"]
mod saves;
#[path = "../../silico-core/tests/common/share.rs"]
mod share;
#[path = "common/systems.rs"]
mod systems;
#[path = "common/wait.rs"]
mod wait;

#[path = "adversarial_drag.rs"]
mod adversarial_drag;
#[path = "adversarial_drag_session.rs"]
mod adversarial_drag_session;
#[path = "adversarial_load_path.rs"]
mod adversarial_load_path;
#[path = "adversarial_stored_coordinates.rs"]
mod adversarial_stored_coordinates;
#[path = "document.rs"]
mod document;
#[path = "drag.rs"]
mod drag;
#[path = "worker.rs"]
mod worker;
