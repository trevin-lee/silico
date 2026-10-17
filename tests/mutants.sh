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
# off; a test bounded by CPU time (silico-core's tests/common/within.rs), or by events and
# the times its workers log, already is, and stays. What they alone test, so a survivor
# there is checked against them, unloaded, before it's a finding: that a name reported for
# many elements is written once and stays within its size bound (silico-core's
# naming/build.rs, MOST_WEIGHT, which names_that_would_double_... tests too).
core_wall_clock=(
  a_name_reported_for_many_elements_is_written_once
)
# package(p) & (test(=a) | test(=b) | ...), for a package and its tests.
tests_of() {
  local package=$1 tests="" t
  shift
  for t in "$@"; do tests="${tests:+$tests | }test(=$t)"; done
  echo "package($package) & ($tests)"
}
filter="not ($(tests_of silico-core "${core_wall_clock[@]}"))"
cargo mutants --in-place --minimum-test-timeout 300 --cargo-test-arg=-E --cargo-test-arg="$filter" "$@"
