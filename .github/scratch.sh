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

case "${1-}" in
  start)
    # A job the runner itself lost never ran `end`: its scratch is removed once it's older
    # than the longest job (mutants', a day) could be, by twice.
    find "$root" -maxdepth 1 -name 'silico-*' -user "$(id -u)" -mmin +2880 \
      -exec rm -rf {} + 2>/dev/null || true
    mkdir -p "$scratch/tmp" "$scratch/target-tmp"
    {
      echo "TMPDIR=$scratch/tmp"
      echo "SILICO_TEST_SCRATCH=$scratch/target-tmp"
    } >>"$GITHUB_ENV"
    # Its peak size, sampled every 5 s until `end` removes it.
    (
      peak=0
      while [ -d "$scratch" ]; do
        kib=$(du -sk "$scratch" 2>/dev/null | cut -f1) || kib=0
        if [ "${kib:-0}" -gt "$peak" ]; then
          peak=$kib
          echo "$peak" >"$scratch.peak"
        fi
        sleep 5
      done
    ) >/dev/null 2>&1 &
    disown
    echo "Test scratch in memory: $scratch ($(df -h --output=avail "$root" | tail -1 | tr -d ' ') free on $root)"
    ;;
  end)
    peak=$(cat "$scratch.peak" 2>/dev/null || echo 0)
    now=$(du -sk "$scratch" 2>/dev/null | cut -f1) || now=0
    if [ "${now:-0}" -gt "$peak" ]; then peak=$now; fi
    line="Test scratch in memory: peak $((peak / 1024)) MiB (sampled every 5 s)"
    echo "$line"
    echo "$line" >>"${GITHUB_STEP_SUMMARY:-/dev/null}"
    rm -rf "$scratch" "$scratch.peak"
    ;;
  *)
    echo "usage: $0 start|end" >&2
    exit 2
    ;;
esac
