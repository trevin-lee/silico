//! silico-kernel's geometry tests, as one test program: each file a module, its tests
//! as they were, the modules they share loaded once: fewer programs to build and link than
//! one a file, for every change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/box_edges.rs"]
mod box_edges;
#[path = "common/brep_fixture.rs"]
mod brep_fixture;
#[path = "common/cached.rs"]
mod cached;
#[path = "common/chamfer_sizes.rs"]
mod chamfer_sizes;
#[path = "common/close.rs"]
mod close;
#[path = "../../silico-core/tests/common/profile.rs"]
mod draw;
#[path = "common/exported.rs"]
mod exported;
#[path = "common/face_where.rs"]
mod face_where;
#[path = "common/facts.rs"]
mod facts;
#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/mesh_geometry.rs"]
mod geometry;
#[path = "../../silico-core/tests/common/hostile_profiles.rs"]
mod hostile;
#[path = "common/imports.rs"]
mod imports;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/oracle.rs"]
mod oracle;
#[path = "common/quadrature.rs"]
mod quadrature;
#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

#[path = "adversarial_arc_range.rs"]
mod adversarial_arc_range;
#[path = "adversarial_boolean.rs"]
mod adversarial_boolean;
#[path = "adversarial_boolean_skipped.rs"]
mod adversarial_boolean_skipped;
#[path = "adversarial_chamfer.rs"]
mod adversarial_chamfer;
#[path = "adversarial_chamfer_report.rs"]
mod adversarial_chamfer_report;
#[path = "adversarial_classify.rs"]
mod adversarial_classify;
#[path = "adversarial_describe.rs"]
mod adversarial_describe;
#[path = "adversarial_distance.rs"]
mod adversarial_distance;
#[path = "adversarial_export.rs"]
mod adversarial_export;
#[path = "adversarial_faces.rs"]
mod adversarial_faces;
#[path = "adversarial_fillet.rs"]
mod adversarial_fillet;
#[path = "adversarial_mesh.rs"]
mod adversarial_mesh;
#[path = "adversarial_profile_faces.rs"]
mod adversarial_profile_faces;
#[path = "adversarial_revolution_measures.rs"]
mod adversarial_revolution_measures;
#[path = "adversarial_sketch_feature_build.rs"]
mod adversarial_sketch_feature_build;
#[path = "adversarial_sweep.rs"]
mod adversarial_sweep;
#[path = "boolean.rs"]
mod boolean;
#[path = "boolean_solids.rs"]
mod boolean_solids;
#[path = "bridge.rs"]
mod bridge;
#[path = "chamfer.rs"]
mod chamfer;
#[path = "classify.rs"]
mod classify;
#[path = "describe.rs"]
mod describe;
#[path = "distance.rs"]
mod distance;
#[path = "export.rs"]
mod export;
#[path = "faces.rs"]
mod faces;
#[path = "fillet.rs"]
mod fillet;
#[path = "finite.rs"]
mod finite;
#[path = "mesh.rs"]
mod mesh;
#[path = "mesh_floor.rs"]
mod mesh_floor;
#[path = "primitive_numbers.rs"]
mod primitive_numbers;
#[path = "primitives.rs"]
mod primitives;
#[path = "profile_faces.rs"]
mod profile_faces;
#[path = "revolution_measures.rs"]
mod revolution_measures;
#[path = "sweep.rs"]
mod sweep;
#[path = "thin_wall.rs"]
mod thin_wall;
