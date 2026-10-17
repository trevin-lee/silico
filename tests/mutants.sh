#!/usr/bin/env bash
# Mutation testing (cargo-mutants) on the committed tree, in the gate's worktree
# (tests/committed-tree.sh), so it starts from the gate's build and never edits a checkout
# someone is working in. cargo-mutants has to run in place: a copy of the tree can't find
# OCCT, so every mutant would look unviable. .cargo/mutants.toml makes each mutant run every
# workspace test. If a run is killed mid-mutant, the next gate or mutants run restores the
# tree.
#
# Arguments go to cargo mutants; paths in them must be absolute, since it runs in the
# worktree. Without --output, the results (mutants.out) land in the worktree, which the next
# gate or mutants run cleans: pass --output. CI shards it over the runners
# (.github/workflows/mutants.yml).
#
# Run through pixi:  pixi run mutants -- --in-diff /tmp/change.diff -p silico-core --output /tmp
set -euo pipefail

# shellcheck source=tests/committed-tree.sh
source "$(dirname "${BASH_SOURCE[0]}")/committed-tree.sh"

pixi run occt
pixi install -e oracle
echo "mutants: testing $commit"
# Without the tests that hold to a wall clock: on a machine the shards saturate they fail
# the unmutated baseline, and a failing baseline stops cargo-mutants before it tests any
# mutant. A filter, not a nextest profile, so trees older than this script take it too
# (a name a tree doesn't have matches nothing). Each is to be made load-tolerant and taken
# off. What they alone test, so a survivor there is checked against them, unloaded, before
# it's a finding: the pool's restart, back-off, and kill-at-launch handling
# (silico-core's pool.rs: killed_unstarted, back-off reset, a newer worker waking a slot),
# a superseded request's stop, and dropping a pool of workers.
wall_clock=(
  a_pool_of_the_most_workers_starts_and_stops_cleanly
  dropping_the_pool_amid_crashes_kills_floods_and_advances_leaves_nothing
  superseded_work_too_young_to_stop_at_advance_is_stopped_once_it_has_run_stop_after
  a_kill_at_launch_after_an_exit_of_its_own_is_taken_as_from_outside
  an_idle_slot_whose_worker_is_killed_at_launch_backs_off_after_the_second_kill
  a_slot_waiting_out_a_long_back_off_starts_a_worker_once_a_newer_one_has_started
  a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once
)
filter="package(silico-kernel)"
skipped=""
for t in "${wall_clock[@]}"; do skipped="${skipped:+$skipped | }test(=$t)"; done
filter="not ($filter & ($skipped))"
cargo mutants --in-place --minimum-test-timeout 300 --cargo-test-arg=-E --cargo-test-arg="$filter" "$@"
