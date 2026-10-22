#!/usr/bin/env bash
# A CI job's test scratch, in memory: `start` makes a directory for the job on the VM's
# tmpfs (/dev/shm) and points TMPDIR and the tests' target/tmp (SILICO_TEST_SCRATCH, which
# tests/committed-tree.sh links target/tmp to) into it; `end` reports how large it grew and
# removes it. On the VM's disk, which every runner's builds and tests share, a test's fsync
# stalled for 70 to 102 s with three gates writing at once (#75), past nextest's limit.
# Build outputs stay on disk.
#
# Usage: bash .github/scratch.sh start   (a step before the tests)
#        bash .github/scratch.sh end     (a step after them, `if: always()`)
set -euo pipefail

root=/dev/shm
scratch="$root/silico-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-$RUNNER_NAME"
# What one job may hold before it warns: the VM has 70 GB and no swap, shared with other
# projects' runners, and its two tmpfs (this and /tmp) may each grow to 48 GB.
warn_kib=$((8 * 1024 * 1024))

case "${1-}" in
  start)
    # A job whose `end` never ran (its runner died, or it was force-cancelled) leaves its
    # scratch holding memory: removed here once it's older than any job can last (mutants',
    # a day), plus an hour.
    find "$root" -maxdepth 1 -name 'silico-*' -user "$(id -u)" -mmin +1500 \
      -exec rm -rf {} + 2>/dev/null || true
    mkdir -p "$scratch/tmp" "$scratch/target-tmp"
    {
      echo "TMPDIR=$scratch/tmp"
      echo "SILICO_TEST_SCRATCH=$scratch/target-tmp"
    } >>"$GITHUB_ENV"
    # Until `end` removes it: its peak size, sampled every 5 s; and, every minute, whatever
    # tests left behind over an hour ago, which no test still running can be using (a
    # mutant's tests get 20 minutes), so a long mutants shard's leftovers don't build up in
    # memory. A write after `end` fails, the directory being gone, and ends the loop.
    (
      peak=0
      tick=0
      while [ -d "$scratch" ]; do
        kib=$(du -sk "$scratch" 2>/dev/null | cut -f1) || true
        if [ "${kib:-0}" -gt "$peak" ]; then
          peak=$kib
          echo "$peak" >"$scratch/peak" || exit 0
        fi
        tick=$((tick + 1))
        if [ $((tick % 12)) = 0 ]; then
          find "$scratch/tmp" "$scratch/target-tmp" -mindepth 1 -maxdepth 1 -mmin +60 \
            -exec rm -rf {} + 2>/dev/null || true
        fi
        sleep 5
      done
    ) >/dev/null 2>&1 &
    disown
    echo "Test scratch in memory: $scratch ($(df -h --output=avail "$root" | tail -1 | tr -d ' ') free on $root)"
    ;;
  end)
    peak=$(cat "$scratch/peak" 2>/dev/null || echo 0)
    now=$(du -sk "$scratch" 2>/dev/null | cut -f1) || true
    if [ "${now:-0}" -gt "$peak" ]; then peak=$now; fi
    line="Test scratch in memory: peak $((peak / 1024)) MiB (sampled every 5 s)"
    echo "$line"
    echo "$line" >>"${GITHUB_STEP_SUMMARY:-/dev/null}"
    if [ "$peak" -gt "$warn_kib" ]; then
      echo "::warning::test scratch grew past $((warn_kib / 1024 / 1024)) GiB in memory; look for tests that leave theirs behind"
    fi
    rm -rf "$scratch"
    ;;
  *)
    echo "usage: $0 start|end" >&2
    exit 2
    ;;
esac
