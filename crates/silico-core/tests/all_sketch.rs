//! silico-core's sketches' tests (their format, gate, solving, features, and stored
//! coordinates), as one test program: each file a module, its tests as they were, the
//! modules they share loaded once: fewer programs to build and link than one a file, for
//! every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/bracket.rs"]
mod bracket;
#[path = "common/command.rs"]
mod commands;
#[path = "common/example.rs"]
mod example;
#[path = "common/sketch.rs"]
mod fixture;
#[path = "common/lateness.rs"]
mod lateness;
#[path = "common/model.rs"]
mod model;
#[path = "common/share.rs"]
mod share;
#[path = "common/stand_in.rs"]
mod stand_in;
#[path = "common/sweeps.rs"]
mod sweeps;
#[path = "common/within.rs"]
mod within;

#[path = "adversarial_decision5_parts.rs"]
mod adversarial_decision5_parts;
#[path = "adversarial_drag_commands.rs"]
mod adversarial_drag_commands;
#[path = "adversarial_drag_session.rs"]
mod adversarial_drag_session;
#[path = "adversarial_load_path.rs"]
mod adversarial_load_path;
#[path = "adversarial_redundant_resolve.rs"]
mod adversarial_redundant_resolve;
#[path = "adversarial_sketch.rs"]
mod adversarial_sketch;
#[path = "adversarial_sketch_add.rs"]
mod adversarial_sketch_add;
#[path = "adversarial_sketch_drag.rs"]
mod adversarial_sketch_drag;
#[path = "adversarial_sketch_edit.rs"]
mod adversarial_sketch_edit;
#[path = "adversarial_sketch_extend.rs"]
mod adversarial_sketch_extend;
#[path = "adversarial_sketch_extend_to.rs"]
mod adversarial_sketch_extend_to;
#[path = "adversarial_sketch_features.rs"]
mod adversarial_sketch_features;
#[path = "adversarial_sketch_merge_tangent.rs"]
mod adversarial_sketch_merge_tangent;
#[path = "adversarial_sketch_solve.rs"]
mod adversarial_sketch_solve;
#[path = "adversarial_sketch_split.rs"]
mod adversarial_sketch_split;
#[path = "adversarial_sketch_states.rs"]
mod adversarial_sketch_states;
#[path = "adversarial_sketch_trim.rs"]
mod adversarial_sketch_trim;
#[path = "adversarial_stored_coordinates.rs"]
mod adversarial_stored_coordinates;
#[path = "adversarial_timeline_commands.rs"]
mod adversarial_timeline_commands;
#[path = "adversarial_warm.rs"]
mod adversarial_warm;
#[path = "chain.rs"]
mod chain;
#[path = "drag_session.rs"]
mod drag_session;
#[path = "session_drag.rs"]
mod session_drag;
#[path = "sketch.rs"]
mod sketch;
#[path = "sketch_add.rs"]
mod sketch_add;
#[path = "sketch_drag.rs"]
mod sketch_drag;
#[path = "sketch_edit.rs"]
mod sketch_edit;
#[path = "sketch_extend.rs"]
mod sketch_extend;
#[path = "sketch_extend_to.rs"]
mod sketch_extend_to;
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
#[path = "sketch_states.rs"]
mod sketch_states;
#[path = "sketch_trim.rs"]
mod sketch_trim;
#[path = "stored_coordinates.rs"]
mod stored_coordinates;
#[path = "timeline.rs"]
mod timeline;
#[path = "undo.rs"]
mod undo;
#[path = "wire_timeline_commands.rs"]
mod wire_timeline_commands;
