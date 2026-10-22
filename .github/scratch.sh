#!/usr/bin/env bash
# A CI job's test scratch, in memory (a gate's, or a mutants shard's): `start` makes a directory for the job on the VM's
# tmpfs (/dev/shm) and points TMPDIR and the tests' target/tmp (SILICO_TEST_SCRATCH, which
# tests/committed-tree.sh links target/tmp to) into it; `end` reports how large it grew,
# puts target/tmp back as a directory, and removes it. On the VM's disk, which every runner's builds and tests share, a test's fsync
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

# Removes scratch, made writable first: a test can leave a directory it made read-only
# behind (cache_bound's read-only cache), whose entries rm can't unlink.
remove() {
  chmod -R u+w "$@" 2>/dev/null || true
  rm -rf "$@"
}

case "${1-}" in
  start)
    # A job whose `end` never ran (its runner died, or it was force-cancelled) leaves its
    # scratch holding memory: removed here once it's over 25 hours old, past the longest
    # job's limit (a mutants shard's, a day).
    find "$root" -maxdepth 1 -name 'silico-*' -user "$(id -u)" -mmin +1500 -print0 2>/dev/null |
      while IFS= read -r -d '' lost; do remove "$lost" || true; done
    # And a gate tree's target/tmp left linked to scratch that's gone (the job lost, its
    # scratch swept) is put back as an empty directory, as `end` does: a running job's
    # link always leads to a directory. Only under the tree's own lock (committed-tree.sh
    # holds it for a whole gate or mutants run), so a job that's linking it isn't raced.
    for link in "${XDG_CACHE_HOME:-$HOME/.cache}"/silico/gate/*/target/tmp; do
      if [ -L "$link" ] && [ ! -e "$link" ]; then
        flock -n "${link%/target/tmp}.lock" \
          sh -c 'if [ -L "$1" ] && [ ! -e "$1" ]; then rm -f "$1"; mkdir -p "$1"; fi' _ "$link" ||
          true
      fi
    done
    mkdir -p "$scratch/tmp" "$scratch/target-tmp"
    {
      echo "TMPDIR=$scratch/tmp"
      echo "SILICO_TEST_SCRATCH=$scratch/target-tmp"
    } >>"$GITHUB_ENV"
    # Its peak size, sampled every 5 s until `end` removes it. What tests leave behind stays
    # until then (nothing is pruned mid-run: a later test may read what an earlier one
    # made): a gate's is a few hundred MiB, a long mutants shard's a few GiB. A write after
    # `end` fails, the directory being gone, and ends the loop.
    (
      peak=0
      while [ -d "$scratch" ]; do
        kib=$(du -sk "$scratch" 2>/dev/null | cut -f1) || true
        if [ "${kib:-0}" -gt "$peak" ]; then
          peak=$kib
          echo "$peak" >"$scratch/peak" || exit 0
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
    # The gate trees' target/tmp linked into this scratch (tests/committed-tree.sh) is put
    # back as an empty directory, so the next job finds a plain tree whatever its scripts'
    # age: a tree checked out from before the link existed fails at its first cargo step
    # on a dangling one ("failed to create directory .../target/tmp: File exists").
    for link in "${XDG_CACHE_HOME:-$HOME/.cache}"/silico/gate/*/target/tmp; do
      if [ -L "$link" ] && [ "$(readlink "$link")" = "$scratch/target-tmp" ]; then
        rm -f "$link"
        mkdir -p "$link" || true
      fi
    done
    if [ "$peak" -gt "$warn_kib" ]; then
      echo "::warning::test scratch grew past $((warn_kib / 1024 / 1024)) GiB in memory; look for tests that leave theirs behind"
    fi
    remove "$scratch"
    ;;
  *)
    echo "usage: $0 start|end" >&2
    exit 2
    ;;
esac
