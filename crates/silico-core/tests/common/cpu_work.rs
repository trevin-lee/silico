//! How much CPU a fixed piece of work of a recompute's kind costs this process now: what a
//! test divides a recompute's CPU time by, so its bound doesn't depend on how fast this
//! machine's CPU does such work (a slower or busier runner, a core shared with another
//! thread, memory pressure), which `super::share`'s slowdown can't see: that measures how
//! long work waits for a CPU, and waiting costs no CPU time.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::hint::black_box;
use std::time::Duration;

/// The entries the work's map holds at most, and the rounds it fills and trims it.
const ENTRIES: usize = 20_000;
const ROUNDS: usize = 5;

/// The CPU this thread takes to do the same work every time: buffers allocated and filled,
/// hashed, and kept in a `BTreeMap` by their hashes, half of them dropped each round, as the
/// fake kernel's shapes and the recompute's keys and maps are.
pub fn cost() -> Duration {
    let cpu = cpu_time::ThreadTime::now();
    let mut map: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for round in 0..ROUNDS {
        for i in 0..ENTRIES {
            let mut bytes = Vec::with_capacity(64);
            for word in 0..8_u64 {
                bytes.extend_from_slice(&((i as u64) ^ (round as u64) << 32 ^ word).to_le_bytes());
            }
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            map.insert(hasher.finish(), bytes);
        }
        let dropped: Vec<u64> = map.keys().step_by(2).copied().collect();
        for key in dropped {
            map.remove(&key);
        }
    }
    black_box(&map);
    cpu.elapsed()
}
