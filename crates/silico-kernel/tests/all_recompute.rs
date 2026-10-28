//! silico-kernel's recompute tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
//!
//! Some install the process's global tracing subscriber, or measure the process's CPU time:
//! each test needs a process of its own, as nextest runs each in (`pixi run test`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/cached.rs"]
mod cached;
#[path = "../../silico-core/tests/common/example.rs"]
mod example;
#[path = "../../silico-core/tests/common/lateness.rs"]
mod lateness;
#[path = "common/measuring.rs"]
mod measuring;
#[path = "../../silico-core/tests/common/model.rs"]
mod model;
#[path = "common/running.rs"]
mod running;
#[path = "common/scripts.rs"]
mod scripts;
#[path = "common/wait.rs"]
mod wait;

#[path = "adversarial_cache.rs"]
mod adversarial_cache;
#[path = "adversarial_center_to_element.rs"]
mod adversarial_center_to_element;
#[path = "adversarial_entries.rs"]
mod adversarial_entries;
#[path = "adversarial_fake_kernel.rs"]
mod adversarial_fake_kernel;
#[path = "adversarial_fingerprint.rs"]
mod adversarial_fingerprint;
#[path = "adversarial_mass_properties.rs"]
mod adversarial_mass_properties;
#[path = "adversarial_measures.rs"]
mod adversarial_measures;
#[path = "adversarial_read_logs.rs"]
mod adversarial_read_logs;
#[path = "adversarial_recompute.rs"]
mod adversarial_recompute;
#[path = "cache.rs"]
mod cache;
#[path = "elements.rs"]
mod elements;
#[path = "entries.rs"]
mod entries;
#[path = "fake_kernel.rs"]
mod fake_kernel;
#[path = "fingerprint.rs"]
mod fingerprint;
#[path = "mass_properties.rs"]
mod mass_properties;
#[path = "measures.rs"]
mod measures;
#[path = "meshes_read.rs"]
mod meshes_read;
#[path = "recompute.rs"]
mod recompute;
#[path = "recompute_spans_close.rs"]
mod recompute_spans_close;
#[path = "recompute_tracing.rs"]
mod recompute_tracing;
#[path = "recompute_tracing_lagging.rs"]
mod recompute_tracing_lagging;
#[path = "refused_span_closes.rs"]
mod refused_span_closes;
#[path = "session_logs.rs"]
mod session_logs;
#[path = "wire_examples.rs"]
mod wire_examples;
