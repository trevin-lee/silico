//! silico-core's document tests (documents, commands and sessions, fingerprints, references,
//! type words, and the cache, with the result checks the cache shares), as one test program:
//! each file a module, its tests as they were, the modules they share loaded once: fewer
//! programs to build and link than one a file, for every change (each mutant's build most of
//! all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/bound.rs"]
mod bound;
#[path = "common/cache.rs"]
mod cache;
#[path = "common/cache_format.rs"]
mod cache_format;
#[path = "common/command.rs"]
mod commands;
#[path = "common/example.rs"]
mod example;
#[path = "common/forged.rs"]
mod forged;
#[path = "common/lateness.rs"]
mod lateness;
#[path = "common/entries.rs"]
mod listing;
#[path = "common/model.rs"]
mod model;
#[path = "common/references.rs"]
mod references;
#[path = "common/session.rs"]
mod sessions;
#[path = "common/sweeps.rs"]
mod sweeps;
#[path = "common/within.rs"]
mod within;

#[path = "common/test_programs.rs"]
mod test_programs;

#[path = "adversarial_cache.rs"]
mod adversarial_cache;
#[path = "adversarial_cache_bound.rs"]
mod adversarial_cache_bound;
#[path = "adversarial_center_to_element.rs"]
mod adversarial_center_to_element;
#[path = "adversarial_command.rs"]
mod adversarial_command;
#[path = "adversarial_document.rs"]
mod adversarial_document;
#[path = "adversarial_fingerprint.rs"]
mod adversarial_fingerprint;
#[path = "adversarial_mass_properties.rs"]
mod adversarial_mass_properties;
#[path = "adversarial_measures.rs"]
mod adversarial_measures;
#[path = "adversarial_references.rs"]
mod adversarial_references;
#[path = "adversarial_type_words.rs"]
mod adversarial_type_words;
#[path = "adversarial_wire_commands.rs"]
mod adversarial_wire_commands;
#[path = "cache_bound.rs"]
mod cache_bound;
#[path = "command.rs"]
mod command;
#[path = "document.rs"]
mod document;
#[path = "fingerprint.rs"]
mod fingerprint;
#[path = "mass_properties.rs"]
mod mass_properties;
#[path = "measures.rs"]
mod measures;
#[path = "reach.rs"]
mod reach;
#[path = "refused_from_a_partial_cache.rs"]
mod refused_from_a_partial_cache;
#[path = "session.rs"]
mod session;
#[path = "session_cancel.rs"]
mod session_cancel;
#[path = "session_edits.rs"]
mod session_edits;
#[path = "session_elements.rs"]
mod session_elements;
#[path = "session_rebuild.rs"]
mod session_rebuild;
#[path = "session_reload.rs"]
mod session_reload;
#[path = "session_tracing.rs"]
mod session_tracing;
#[path = "session_transactions.rs"]
mod session_transactions;
#[path = "shared_result_checks.rs"]
mod shared_result_checks;
#[path = "wire_commands.rs"]
mod wire_commands;
#[path = "wire_session_commands.rs"]
mod wire_session_commands;
#[path = "wire_session_reads.rs"]
mod wire_session_reads;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
