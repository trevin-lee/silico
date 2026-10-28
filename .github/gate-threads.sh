#!/usr/bin/env bash
# How many threads a gate takes (.github/workflows/gate.yml): a quarter of the VM, as before,
# unless more gates run at once than four quarters hold: then an even share of all but two of
# the VM's CPUs, min(nproc / 4, (nproc - 2) / gates), at least 2. Six gates at a quarter each
# asked for 42 threads on 30, and their own CPU pressure (each gate service's cgroup) held
# 15-16% over five minutes (2026-10-28 02:27-02:32Z, beside nine mutants shards at the idle
# weight), their test phases 11-12% slower than two gates' (~/.cache/silico-review/w2 on the
# maintainer's Mac). Main's rule from the 2026-10-27 re-sample: past 10% over five minutes,
# scale gates' threads by how many run.
#
# Each gate holds a claim, a file named for its runner instance, from its start to its job's end
# (`release`, an `if: always()` step), under the same lock as the mutants shards' claims
# (.github/shard-threads.sh), so gates starting together count each other. A gate keeps the
# count it chose to its end: one starting into a crowd takes less, and those already running
# keep theirs. A runner runs one job at a time, so a claim its job left (a runner that died) is
# replaced by its next gate's; claims older than a gate's longest job (two hours) are deleted.
#
# Usage: bash .github/gate-threads.sh            (prints the count, then how it was chosen)
#        bash .github/gate-threads.sh release    (at the job's end)
set -euo pipefail

claims=$HOME/.cache/silico/shard-threads
mkdir -p "$claims"
# By instance (pve-silico-3), not by registration (pve-silico-3-1793154549): each job's runner
# registers again under a new suffix, so only the instance's name is the same for its next gate.
# Only a numeric suffix comes off: a name without one stays whole, its own claim.
instance=$RUNNER_NAME
if [[ $RUNNER_NAME =~ ^(.*)-[0-9]+$ ]]; then instance=${BASH_REMATCH[1]}; fi
claim=$claims/gate.$instance

exec 9>"$claims/lock"
# A minute at most: shards hold the lock through their 5 s sample, a long run's seven in turn.
# Past that, fail, and the gate takes the quarter (gate.yml).
flock -w 60 9

if [ "${1:-}" = release ]; then
  rm -f "$claim"
  exit 0
fi

cpus=$(nproc)
quarter=$((cpus / 4 > 2 ? cpus / 4 : 2))
find "$claims" -name 'gate.*' -mmin +120 -delete
echo 1 >"$claim"
gates=$(find "$claims" -name 'gate.*' | wc -l)
even=$(((cpus - 2) / gates))
threads=$((even < quarter ? even : quarter))
threads=$((threads < 2 ? 2 : threads))

echo "$threads"
echo "$threads threads: $gates gates running, a quarter of $cpus is $quarter, an even share of $((cpus - 2)) is $even"
