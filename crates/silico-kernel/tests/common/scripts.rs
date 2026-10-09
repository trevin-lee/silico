//! Stand-in workers written as shell scripts, for tests of how the core treats a worker
//! that misbehaves.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// An executable `/bin/sh` script running `body`, in the test's scratch directory.
pub fn script(name: &str, body: &str) -> PathBuf {
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}.sh", std::process::id()));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
