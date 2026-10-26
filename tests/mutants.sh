#!/usr/bin/env bash
# Mutation testing (cargo-mutants) on the committed tree, in the gate's worktree
# (tests/committed-tree.sh), so it never edits a checkout someone is working in. Off CI it
# starts from the gate's build; on CI it keeps its own, without debug info, beside it
# (SILICO_TARGET, .github/workflows/mutants.yml). cargo-mutants has to run in place: a copy of the tree can't find
# OCCT, so every mutant would look unviable. .cargo/mutants.toml makes each mutant run every
# workspace test. If a run is killed mid-mutant, the next gate or mutants run restores the
# tree.
#
# Arguments go to cargo mutants; paths to files (--in-diff, --output) must be absolute,
# since it runs in the worktree, while --file names source files from the tree's root. Without --output, the results (mutants.out) land in the worktree, which the next
# gate or mutants run cleans: pass --output. CI shards it over the runners
# (.github/workflows/mutants.yml). The test timeout is set below: --timeout-multiplier
# conflicts with it, and --minimum-test-timeout does nothing.
#
# Run through pixi:  pixi run mutants -- --in-diff /tmp/change.diff -p silico-core --output /tmp
set -euo pipefail

# shellcheck source=tests/committed-tree.sh
source "$(dirname "${BASH_SOURCE[0]}")/committed-tree.sh"

pixi run occt
pixi install -e oracle
# cargo-mutants' baseline builds and tests only the mutated packages, and their tests start
# other packages' workers (silico-oracle's start the kernel's): build the whole workspace's
# tests first, as the gate and each mutant build them, since the build may start cold
# (committed-tree.sh).
cargo nextest run --workspace --no-run
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
  a_name_reported_for_many_elements_is_written_once_promptly
)
# The largest sketch no algorithm solves, bounded by its wall clock (a minute or two loaded,
# against SKETCH_TIMEOUT): what it alone tests is that the worker's time covers every
# algorithm at MAX_SKETCH_UNKNOWNS; that the time is SketchWorker's own is tested apart
# (silico-sketch's document.rs).
sketch_wall_clock=(
  an_impossible_chain_of_the_largest_size_fails_as_solve_failed_not_a_timeout
)
# The precise measure of a 400-span B-spline meridian's turn, and of a rough 1000-span
# B-spline face trimmed to a triangle (OCCT patch 0004's search of only a strip's own
# intervals), each bounded by its wall clock (10 s and 30 s, under the worker's 60 s timeout): what
# they alone test is OCCT's, that the measure finishes and gives its known values; Silico's
# measure path, which they run, is tested by every other measure test. Named with their
# modules, as all_geometry's tests are.
kernel_wall_clock=(
  adversarial_revolution_measures::a_bspline_meridian_of_many_spans_turned_measures_as_pappus
  adversarial_measure_refine_u::a_rough_bspline_face_of_a_thousand_spans_trimmed_to_a_triangle_measures_in_time
)
# The render worker's scale tests, left out: each holds GiBs (.config/nextest.toml's render
# group), which every shard at once would. Their mutants are in silico-render, where its
# small adversarial tests cover them.
render_scale=(
  adversarial_render::the_most_edges_a_scene_may_have_draw_within_the_limit_and_one_more_is_refused
  adversarial_render::the_most_faces_a_scene_may_have_draw_within_the_limit_and_one_more_is_refused
  adversarial_render::a_part_of_ctc_size_at_4096_square_with_highlights_draws_within_the_limit
  adversarial_render::three_thousand_bodies_draw_within_half_the_limit_each_with_its_number
  adversarial_render::ten_thousand_highlighted_faces_draw_within_half_the_limit
  worker::the_largest_render_is_the_box_from_the_front_too
)
# package(p) & (test(=a) | test(=b) | ...), for a package and its tests.
tests_of() {
  local package=$1 tests="" t
  shift
  for t in "$@"; do tests="${tests:+$tests | }test(=$t)"; done
  echo "package($package) & ($tests)"
}
filter="not ($(tests_of silico-core "${core_wall_clock[@]}") | $(tests_of silico-sketch "${sketch_wall_clock[@]}") | $(tests_of silico-kernel "${kernel_wall_clock[@]}") | $(tests_of silico-render "${render_scale[@]}"))"
# Each test given 5 minutes a try, not the 2 its slow-timeout gives (.config/nextest.toml):
# the shards run niced, and while gates build and test at full priority a niced test that
# takes 20 s alone can take minutes. Ended at 2, under retries no longer than a burst of
# gates, its failure would count the mutant as caught: a false kill, which hides a miss. A
# mutant that hangs a test is still caught: three tries of 5 minutes end inside the 30
# below. The tree's own config with a profile added (tests that allow longer keep it), by
# --config-file, so trees older than this script take it too.
nextest_config=$(mktemp "${TMPDIR:-/tmp}/silico-mutants-nextest.XXXXXX")
trap 'rm -f "$nextest_config"' EXIT
{
  cat .config/nextest.toml
  printf '\n[profile.silico-mutants]\nslow-timeout = { period = "60s", terminate-after = 5 }\n'
} >"$nextest_config"
# Each mutant's tests, and the baseline's, given 30 minutes: no multiple of the baseline,
# which runs only the mutated packages' tests. The workspace's take 4 to 5 minutes on a
# loaded runner, and Cargo can wait as long again before them on the package cache's lock,
# which every runner on the VM shares. (At 300 s, every test had passed so far in 10 of
# run 39630661896's 11 timeouts.) A hung test, retried twice on CI, takes 15 minutes from
# when it starts, at most about 5 into the run: nextest ends it first, and its failure
# catches the mutant. A run past 30 minutes times out, and a timeout is triaged as missed.
cargo mutants --in-place --timeout 1800 --cargo-test-arg=-E --cargo-test-arg="$filter" \
  --cargo-test-arg=--config-file --cargo-test-arg="$nextest_config" \
  --cargo-test-arg=--profile --cargo-test-arg=silico-mutants "$@"
