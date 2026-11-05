//! silico-core's recompute tests (recompute and its tracing, naming, measures and their
//! refusals, validation, and saves' tracing), as one test program: each file a module, its
//! tests as they were, the modules they share loaded once: fewer programs to build and link
//! than one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/cache.rs"]
mod cache;
#[path = "common/cache_format.rs"]
mod cache_format;
#[path = "common/cpu_work.rs"]
mod cpu_work;
#[path = "common/describing.rs"]
mod describing;
#[path = "common/example.rs"]
mod example;
#[path = "common/lateness.rs"]
mod lateness;
#[path = "common/entries.rs"]
mod listing;
#[path = "common/model.rs"]
mod model;
#[path = "common/recompute.rs"]
mod recompute;
#[path = "common/refusals.rs"]
mod refusals;
#[path = "common/share.rs"]
mod share;
#[path = "common/sweeps.rs"]
mod sweeps;

#[path = "adversarial_described.rs"]
mod adversarial_described;
#[path = "adversarial_entries.rs"]
mod adversarial_entries;
#[path = "adversarial_measure_refusal.rs"]
mod adversarial_measure_refusal;
#[path = "adversarial_rebuild_features.rs"]
mod adversarial_rebuild_features;
#[path = "adversarial_recompute.rs"]
mod adversarial_recompute;
#[path = "adversarial_selection.rs"]
mod adversarial_selection;
#[path = "adversarial_validate.rs"]
mod adversarial_validate;
#[path = "described.rs"]
mod described;
#[path = "entries.rs"]
mod entries;
#[path = "measure_refusal.rs"]
mod measure_refusal;
#[path = "name_parse.rs"]
mod name_parse;
#[path = "naming.rs"]
mod naming;
#[path = "naming_header.rs"]
mod naming_header;
#[path = "naming_scheme.rs"]
mod naming_scheme;
#[path = "recompute_naming_cost.rs"]
mod recompute_naming_cost;
#[path = "recompute_steps.rs"]
mod recompute_steps;
#[path = "recompute_tracing.rs"]
mod recompute_tracing;
#[path = "recompute_tracing_warn.rs"]
mod recompute_tracing_warn;
#[path = "save_tracing.rs"]
mod save_tracing;
#[path = "shared_step_verdicts.rs"]
mod shared_step_verdicts;
#[path = "validate.rs"]
mod validate;
