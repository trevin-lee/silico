//! For the adversarial tests: scratch directories, and this process's descriptors and
//! threads.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh directory under the target's scratch space, named for the test binary.
pub fn scratch_dir(what: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{}-{what}-{}-{}",
        env!("CARGO_CRATE_NAME"),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Open descriptors in this process (the listing's own included).
pub fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

/// Threads in this process.
///
/// On Linux, the count the kernel keeps (`Threads:` in `/proc/self/status`), never a
/// listing of `/proc/self/task`: a listing that meets a thread as it exits stops there,
/// and the next read resumes by position in the shorter list, so it misses a live thread
/// listed after the one that went. A test waiting for its threads to go would read that
/// short count as them gone. On the CI VM, the pool's drop race counted 3 where 4 were
/// live in 6 of 1,200 runs under load.
pub fn threads() -> usize {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        return status
            .lines()
            .find_map(|line| line.strip_prefix("Threads:"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
    }
    let out = Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    // A header line, then one line per thread.
    String::from_utf8(out.stdout).unwrap().lines().count() - 1
}
