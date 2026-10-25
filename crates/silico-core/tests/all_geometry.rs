//! silico-core's geometry tests (profiles, sweeps, the fake kernel, the pool, refusals, and
//! errors), as one test program: each file a module, its tests as they were, the modules
//! they share loaded once: fewer programs to build and link than one a file, for every
//! change (each mutant's build most of all).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Shared by the files below, each loaded once.
#[path = "common/profile.rs"]
mod draw;
#[path = "common/echo.rs"]
mod echo;
#[path = "common/export_report.rs"]
mod export_report;
#[path = "common/sketch.rs"]
mod fixture;
#[path = "common/hostile_profiles.rs"]
mod hostile;
#[path = "common/sweeps.rs"]
mod sweeps;

#[path = "adversarial_axis_regions.rs"]
mod adversarial_axis_regions;
#[path = "adversarial_export.rs"]
mod adversarial_export;
#[path = "adversarial_export_unraised.rs"]
mod adversarial_export_unraised;
#[path = "adversarial_fake_kernel.rs"]
mod adversarial_fake_kernel;
#[path = "adversarial_meshes.rs"]
mod adversarial_meshes;
#[path = "adversarial_profile.rs"]
mod adversarial_profile;
#[path = "adversarial_profile_faces.rs"]
mod adversarial_profile_faces;
#[path = "adversarial_sweep_recompute.rs"]
mod adversarial_sweep_recompute;
#[path = "adversarial_sweep_refusals.rs"]
mod adversarial_sweep_refusals;
#[path = "error.rs"]
mod error;
#[path = "export.rs"]
mod export;
#[path = "fake_kernel.rs"]
mod fake_kernel;
#[path = "pool.rs"]
mod pool;
#[path = "profile.rs"]
mod profile;
#[path = "profile_geometry.rs"]
mod profile_geometry;
#[path = "profile_ties.rs"]
mod profile_ties;
#[path = "profile_tolerance.rs"]
mod profile_tolerance;
#[path = "recompute_sweeps.rs"]
mod recompute_sweeps;
#[path = "refusals.rs"]
mod refusals;
#[path = "sweep_axis.rs"]
mod sweep_axis;
#[path = "sweep_names.rs"]
mod sweep_names;
#[path = "sweep_refusals.rs"]
mod sweep_refusals;
