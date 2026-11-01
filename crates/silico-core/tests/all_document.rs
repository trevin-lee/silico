//! silico-core's document tests (documents, commands and sessions, fingerprints, references,
//! type words, and the cache, with the result checks the cache shares), as one test program:
//! each file a module, its tests as they were, the modules they share loaded once: fewer
//! programs to build and link than one a file, for every change (each mutant's build most of
//! all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/bound.rs"]
mod bound;
#[path = "common/bracket.rs"]
mod bracket;
#[path = "common/cache.rs"]
mod cache;
#[path = "common/cache_control.rs"]
mod cache_control;
#[path = "common/cache_format.rs"]
mod cache_format;
#[path = "common/call.rs"]
mod calls;
#[path = "common/command.rs"]
mod commands;
#[path = "common/example.rs"]
mod example;
#[path = "common/export_report.rs"]
mod export_report;
#[path = "common/forged.rs"]
mod forged;
#[path = "common/lateness.rs"]
mod lateness;
#[path = "common/entries.rs"]
mod listing;
#[path = "common/logs.rs"]
mod logged;
#[path = "common/model.rs"]
mod model;
#[path = "common/references.rs"]
mod references;
#[path = "common/session.rs"]
mod sessions;
#[path = "common/share.rs"]
mod share;
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
#[path = "adversarial_cache_control.rs"]
mod adversarial_cache_control;
#[path = "adversarial_call.rs"]
mod adversarial_call;
#[path = "adversarial_center_to_element.rs"]
mod adversarial_center_to_element;
#[path = "adversarial_command.rs"]
mod adversarial_command;
#[path = "adversarial_disk.rs"]
mod adversarial_disk;
#[path = "adversarial_document.rs"]
mod adversarial_document;
#[path = "adversarial_face_plane.rs"]
mod adversarial_face_plane;
#[path = "adversarial_fingerprint.rs"]
mod adversarial_fingerprint;
#[path = "adversarial_mass_properties.rs"]
mod adversarial_mass_properties;
#[path = "adversarial_measures.rs"]
mod adversarial_measures;
#[path = "adversarial_meshes_read.rs"]
mod adversarial_meshes_read;
#[path = "adversarial_meshes_wire.rs"]
mod adversarial_meshes_wire;
#[path = "adversarial_read_logs.rs"]
mod adversarial_read_logs;
#[path = "adversarial_references.rs"]
mod adversarial_references;
#[path = "adversarial_render_command.rs"]
mod adversarial_render_command;
#[path = "adversarial_session_disk.rs"]
mod adversarial_session_disk;
#[path = "adversarial_session_timeline.rs"]
mod adversarial_session_timeline;
#[path = "adversarial_session_undo.rs"]
mod adversarial_session_undo;
#[path = "adversarial_sketch_edit_commands.rs"]
mod adversarial_sketch_edit_commands;
#[path = "adversarial_sketch_geometry_edits.rs"]
mod adversarial_sketch_geometry_edits;
#[path = "adversarial_sketch_read.rs"]
mod adversarial_sketch_read;
#[path = "adversarial_timeline.rs"]
mod adversarial_timeline;
#[path = "adversarial_type_words.rs"]
mod adversarial_type_words;
#[path = "adversarial_undo_commands.rs"]
mod adversarial_undo_commands;
#[path = "adversarial_untitled.rs"]
mod adversarial_untitled;
#[path = "adversarial_warnings.rs"]
mod adversarial_warnings;
#[path = "adversarial_wire_commands.rs"]
mod adversarial_wire_commands;
#[path = "adversarial_workspace.rs"]
mod adversarial_workspace;
#[path = "cache_bound.rs"]
mod cache_bound;
#[path = "cache_clear.rs"]
mod cache_clear;
#[path = "cache_workspace.rs"]
mod cache_workspace;
#[path = "call.rs"]
mod call;
#[path = "check_warnings.rs"]
mod check_warnings;
#[path = "command.rs"]
mod command;
#[path = "disk.rs"]
mod disk;
#[path = "document.rs"]
mod document;
#[path = "face_sketch.rs"]
mod face_sketch;
#[path = "face_sketch_add.rs"]
mod face_sketch_add;
#[path = "face_sketch_read.rs"]
mod face_sketch_read;
#[path = "fingerprint.rs"]
mod fingerprint;
#[path = "mass_properties.rs"]
mod mass_properties;
#[path = "measures.rs"]
mod measures;
#[path = "meshes_read.rs"]
mod meshes_read;
#[path = "meshes_read_worker_faults.rs"]
mod meshes_read_worker_faults;
#[path = "reach.rs"]
mod reach;
#[path = "refused_from_a_partial_cache.rs"]
mod refused_from_a_partial_cache;
#[path = "render_command.rs"]
mod render_command;
#[path = "session.rs"]
mod session;
#[path = "session_cancel.rs"]
mod session_cancel;
#[path = "session_edits.rs"]
mod session_edits;
#[path = "session_elements.rs"]
mod session_elements;
#[path = "session_export.rs"]
mod session_export;
#[path = "session_fresh.rs"]
mod session_fresh;
#[path = "session_logs.rs"]
mod session_logs;
#[path = "session_meshes.rs"]
mod session_meshes;
#[path = "session_rebuild.rs"]
mod session_rebuild;
#[path = "session_reload.rs"]
mod session_reload;
#[path = "session_render.rs"]
mod session_render;
#[path = "session_save_as.rs"]
mod session_save_as;
#[path = "session_sketch.rs"]
mod session_sketch;
#[path = "session_sketch_edits.rs"]
mod session_sketch_edits;
#[path = "session_sketch_geometry_edits.rs"]
mod session_sketch_geometry_edits;
#[path = "session_tracing.rs"]
mod session_tracing;
#[path = "session_transactions.rs"]
mod session_transactions;
#[path = "session_untitled.rs"]
mod session_untitled;
#[path = "session_workspace.rs"]
mod session_workspace;
#[path = "shared_result_checks.rs"]
mod shared_result_checks;
#[path = "wire_commands.rs"]
mod wire_commands;
#[path = "wire_meshes.rs"]
mod wire_meshes;
#[path = "wire_session_commands.rs"]
mod wire_session_commands;
#[path = "wire_session_reads.rs"]
mod wire_session_reads;
#[path = "wire_undo_commands.rs"]
mod wire_undo_commands;

/// Every file in tests/ is built: this crate lists its test programs (`autotests = false`),
/// so a new file must be added as a module of one, or as one, or it would never run.
#[test]
fn every_test_file_is_built() {
    test_programs::assert_every_test_file_is_built(env!("CARGO_MANIFEST_DIR"));
}
