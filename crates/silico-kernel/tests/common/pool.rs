//! For the pool's tests: the workers, waiting for outcomes, and watching this process's
//! children (which assumes one test per process, as nextest runs them).

use crate::wait::WAIT;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::{KernelPool, Outcome, Pending};
use silico_protocol::DebugOp;

pub const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
pub const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");

/// Long enough for any request here, so a test that hangs fails instead.
pub const PATIENCE: Duration = Duration::from_secs(60);

pub fn pool(exe: &str, size: usize) -> KernelPool {
    KernelPool::start_workers(Path::new(exe), size).unwrap()
}

pub fn sleep(millis: u64) -> KernelRequest<()> {
    KernelRequest::debug(DebugOp::Sleep { millis })
}

pub fn echo(bytes: &[u8]) -> KernelRequest<Blob> {
    KernelRequest::echo(Blob::inline(bytes.to_vec()))
}

/// The outcome, failing the test if it doesn't come within [`PATIENCE`].
pub fn outcome<T>(pending: Pending<T>) -> Outcome<T> {
    pending
        .wait_timeout(PATIENCE)
        .unwrap_or_else(|_| panic!("no outcome within {PATIENCE:?}"))
}

pub fn done<T>(pending: Pending<T>) -> Result<T, silico_core::Error> {
    outcome(pending).done().expect("superseded")
}

/// This process's children: (pid, state), as `ps` reports them, leaving out `ps` itself.
pub fn children() -> Vec<(u32, String)> {
    let me = std::process::id();
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat="])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let ps_pid = ps.id();
    let out = ps.wait_with_output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            let stat = fields.next()?.to_owned();
            (ppid == me && pid != ps_pid).then_some((pid, stat))
        })
        .collect()
}

/// Checks, at once, that every process in `pids` has been reaped (no process has its ID,
/// not even a zombie), failing with `context` if one hasn't: as every worker a pool held
/// should be by the time its drop returns.
pub fn assert_reaped(pids: &[u32], context: &str) {
    for &pid in pids {
        let pid = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .unwrap();
        assert_eq!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH),
            "{context}: process {pid:?} hasn't been reaped"
        );
    }
}

/// Checks that this process has no children, failing with `context` if it has.
pub fn wait_no_children(context: &str) {
    wait_no_children_besides(&BTreeSet::new(), context);
}

/// Checks that this process has no children but `only` (which may or may not be among
/// them), failing with `context` if it has. `ps` can still list a process reaped an
/// instant before, one it can no longer give a state for (`?E`), on a loaded machine: such
/// a one is given up to [`WAIT`](crate::wait::WAIT) to go. Any other is a process left running, or never
/// reaped, and fails at once.
pub fn wait_no_children_besides(only: &BTreeSet<u32>, context: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left: Vec<_> = children()
            .into_iter()
            .filter(|(pid, _)| !only.contains(pid))
            .collect();
        if left.is_empty() {
            return;
        }
        assert!(
            left.iter().all(|(_, stat)| stat.starts_with('?')),
            "{context}: children not reaped: {left:?}"
        );
        assert!(
            Instant::now() < deadline,
            "{context}: children left: {left:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The process's `ps` state (`S`, `T`, `Z`, ...), or `None` if it's gone.
pub fn state(pid: u32) -> Option<String> {
    children()
        .into_iter()
        .find(|(p, _)| *p == pid)
        .map(|(_, stat)| stat)
}

/// Sends signal `name` (`9`, `STOP`, ...) to `pid` with `kill`.
pub fn signal(name: &str, pid: u32) {
    let status = Command::new("kill")
        .args([&format!("-{name}"), &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "kill -{name} {pid}");
}

/// Kills `pid` with SIGKILL and waits until it has exited (a zombie, as nothing has reaped
/// it yet): a process still exiting can't be told from a live one, so a request given to it
/// in that moment fails as a crash, as any request whose worker is killed under it does.
pub fn kill9_and_wait(pid: u32) {
    signal("9", pid);
    let deadline = Instant::now() + WAIT;
    while state(pid).is_some_and(|s| !s.starts_with('Z')) {
        assert!(Instant::now() < deadline, "{pid} didn't exit");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Busy-waits `d`: `thread::sleep` can't land within a few microseconds.
pub fn spin(d: Duration) {
    let start = Instant::now();
    while start.elapsed() < d {
        std::hint::spin_loop();
    }
}

/// Waits up to [`WAIT`](crate::wait::WAIT) for the pool to hold `size` live workers, none of
/// `gone`.
pub fn wait_full(pool: &KernelPool, size: usize, gone: &BTreeSet<u32>) -> Vec<u32> {
    wait_full_by(pool, size, gone, Instant::now() + WAIT)
}

/// Waits until `deadline` for the pool to hold `size` live workers, none of `gone`.
pub fn wait_full_by(
    pool: &KernelPool,
    size: usize,
    gone: &BTreeSet<u32>,
    deadline: Instant,
) -> Vec<u32> {
    loop {
        let pids = pool.pids();
        let live: BTreeSet<u32> = children()
            .into_iter()
            .filter(|(_, stat)| !stat.starts_with('Z'))
            .map(|(p, _)| p)
            .collect();
        if pids.len() == size
            && pids.iter().all(|p| live.contains(p))
            && pids.iter().all(|p| !gone.contains(p))
        {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "the pool didn't return to {size} live workers: {pids:?} (gone {gone:?})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
