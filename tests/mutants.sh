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
# off; a test bounded by CPU time (silico-core's tests/common/within.rs) already is, and
# stays. What they alone test, so a survivor there is checked against them, unloaded,
# before it's a finding: in silico-kernel, the pool's restart, back-off, and kill-at-launch
# handling (silico-core's pool.rs: killed_unstarted, back-off reset, a newer worker waking
# a slot), a superseded request's stop, and dropping a pool of workers; in silico-core,
# how fast recompute gets through long chains and wide timelines (and through them again
# unchanged), that dropping the engine mid-recompute returns at once, and that naming
# stays linear in the features and the elements named.
kernel_wall_clock=(
  dropping_the_pool_amid_crashes_kills_floods_and_advances_leaves_nothing
  superseded_work_too_young_to_stop_at_advance_is_stopped_once_it_has_run_stop_after
  a_kill_at_launch_after_an_exit_of_its_own_is_taken_as_from_outside
  an_idle_slot_whose_worker_is_killed_at_launch_backs_off_after_the_second_kill
  a_slot_waiting_out_a_long_back_off_starts_a_worker_once_a_newer_one_has_started
  a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once
)
core_wall_clock=(
  a_chain_of_four_hundred_booleans_finishes_promptly
  five_thousand_independent_features_finish_promptly
  dropping_the_engine_mid_recompute_returns_at_once_and_the_run_is_superseded
  a_thousand_holes_drilled_one_by_one_name_in_linear_time
  names_that_would_double_with_every_feature_stay_bounded
  a_name_reported_for_many_elements_is_written_once
)
# package(p) & (test(=a) | test(=b) | ...), for a package and its tests.
tests_of() {
  local package=$1 tests="" t
  shift
  for t in "$@"; do tests="${tests:+$tests | }test(=$t)"; done
  echo "package($package) & ($tests)"
}
filter="not (($(tests_of silico-kernel "${kernel_wall_clock[@]}")) | ($(tests_of silico-core "${core_wall_clock[@]}")))"
cargo mutants --in-place --minimum-test-timeout 300 --cargo-test-arg=-E --cargo-test-arg="$filter" "$@"
