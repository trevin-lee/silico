#!/usr/bin/env bash
# Mutation testing (cargo-mutants) on the committed tree, in the gate's worktree
# (tests/committed-tree.sh), so it never edits a checkout someone is working in. Off CI it
# starts from the gate's build; on CI it keeps its own, without debug info, beside it
# (SILICO_TARGET, .github/workflows/mutants.yml). cargo-mutants has to run in place: a copy of the tree can't find
# OCCT, so every mutant would look unviable. .cargo/mutants.toml makes each mutant run every
# workspace test, unless the run names its tests (--test-package): on CI the plan names the
# mutated packages and those that depend on them, where that's sound (.github/mutants-tests.py,
# mutants.yml's `tests`). If a run is killed mid-mutant, the next gate or mutants run restores
# the tree.
#
# Arguments go to cargo mutants; paths to files (--in-diff, --output) must be absolute,
# since it runs in the worktree, while --file names source files from the tree's root. Without --output, the results (mutants.out) land in the worktree, which the next
# gate or mutants run cleans: pass --output. CI shards it over the runners
# (.github/workflows/mutants.yml). The test timeouts are set below: --timeout would bound
# the baseline too, and cargo-mutants refuses it beside --timeout-multiplier.
#
# Run through pixi:  pixi run mutants -- --in-diff /tmp/change.diff -p silico-core --output /tmp
set -euo pipefail

# Without the tests that hold to a wall clock: on a machine the shards saturate they fail
# the unmutated baseline, and a failing baseline stops cargo-mutants before it tests any
# mutant. A filter, not a nextest profile, so trees older than this script take it too
# (a name a tree doesn't have matches nothing). Each is to be made load-tolerant and taken
# off; a test bounded by CPU time (silico-core's tests/common/within.rs), or by events and
# the times its workers log, already is, and stays. What they alone test, so a survivor
# there is checked against them, unloaded, before it's a finding: that a name reported for
# many elements is written once and stays within its size bound (silico-core's
# naming/build.rs, MOST_WEIGHT, which names_that_would_double_... tests too). The recompute
# tests that held to a wall clock (a chain of four hundred booleans, five thousand
# independent features) now add to it what load costs their work (tests/common/share.rs,
# #207), so they run here; on a loaded shard that allowance grows with the measured
# slowdown, so a recompute.rs survivor that only waits between steps is rechecked unloaded
# before it's a finding.
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
# modules, as all_geometry's tests are. And the 300 mm sphere and torus exported at 0.01,
# 22 s of meshing unloaded and minutes on a starved shard: what they alone test is that the
# export holds a sphere's seam and a large torus to the tolerance at the finest one; the
# rule itself, at every other scale and tolerance, the round-primitives test holds.
kernel_wall_clock=(
  adversarial_revolution_measures::a_bspline_meridian_of_many_spans_turned_measures_as_pappus
  adversarial_measure_refine_u::a_rough_bspline_face_of_a_thousand_spans_trimmed_to_a_triangle_measures_in_time
  adversarial_export::the_largest_sphere_and_torus_at_the_finest_tolerance_are_exported_within_it
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
# silico-render's proptests too slow for a loaded shard (each minutes alone): with them in a
# shard's unmutated baseline it passes the test limit and the shard tests nothing. Left out
# of mutants only: tests/proptest-explore.sh runs them, alone and given longer, so they keep
# getting fresh seeds, and the gate runs them at its fixed one. Beside each entry, say what it
# alone tests (as the lists above do), so a render survivor there is checked against it,
# unloaded, before it's a finding.
slow_proptests=(
  # A thousand random orthographic and perspective looks at long slabs, each view drawn and
  # every clear pixel against a ray cast by hand (some 357 s alone): what it alone tests is
  # the faces' anchors and the cut at random, near the render's limits; each rule it holds
  # is tested at its edge by a fast test of its own.
  adversarial_face_anchors::random_close_looks_at_long_slabs_draw_every_clear_pixel_as_a_ray_cast_by_hand_sees_it
  # The same in perspective alone (some 357 s alone): what it alone tests is the cut of
  # perspective looks at random; the pyramid's tests hold its rules at their edges.
  adversarial_face_anchors::random_close_perspective_looks_at_long_slabs_draw_every_clear_face_pixel_as_a_ray_cast_sees_it
)
# M5.12's scripted slice (silico-app's tests/slice.rs), left out: an end-to-end acceptance
# test that builds the bracket through the app on the real workers and lavapipe, then reads
# its exports back in FreeCAD, minutes on a loaded shard for every mutant; every path it
# drives is covered by the unit and harness tests (the sketch tools', the app's, the core's
# commands, the export's). It stays in the gate.
app_acceptance=(
  the_bracket_built_in_the_app_exports_a_printable_3mf_and_a_step_the_oracle_reads_back
)
# package(p) & (test(=a) | test(=b) | ...), for a package and its tests.
tests_of() {
  local package=$1 tests="" t
  shift
  for t in "$@"; do tests="${tests:+$tests | }test(=$t)"; done
  echo "package($package) & ($tests)"
}
# Left out of mutants and of the exploration alike: the wall clock's tests, the GiB ones, and
# the app's acceptance test.
skipped="$(tests_of silico-core "${core_wall_clock[@]}") | $(tests_of silico-sketch "${sketch_wall_clock[@]}") | $(tests_of silico-kernel "${kernel_wall_clock[@]}") | $(tests_of silico-render "${render_scale[@]}") | $(tests_of silico-app "${app_acceptance[@]}")"
# The slow proptests, for mutants to leave out too and the exploration to run alone: empty
# when there are none, as `package(p) & ()` is no filter.
slow=""
if [ "${#slow_proptests[@]}" -gt 0 ]; then slow=$(tests_of silico-render "${slow_proptests[@]}"); fi
filter="not ($skipped${slow:+ | $slow})"
explore_filter="not ($skipped)"

# tests/proptest-explore.sh sources this file for $explore_filter and $slow alone, and returns
# here, before the tree's checkout.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then return 0; fi

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
# Each test given 5 minutes a try, not the 2 its slow-timeout gives (.config/nextest.toml):
# a loaded machine stretches tests (on the CI VM the shards ran niced beside gates, where a
# test that takes 20 s alone took minutes). Ended at 2, under retries no longer than a burst of
# gates, its failure would count the mutant as caught: a false kill, which hides a miss. A
# mutant that hangs a test is still caught: three tries of 5 minutes end inside the
# 30-minute minimum below. The tree's own config with a profile added (tests that allow
# longer keep it), by --config-file, so trees older than this script take it too.
nextest_config=$(mktemp "${TMPDIR:-/tmp}/silico-mutants-nextest.XXXXXX")
trap 'rm -f "$nextest_config"' EXIT
{
  cat .config/nextest.toml
  printf '\n[profile.silico-mutants]\nslow-timeout = { period = "60s", terminate-after = 5 }\n'
} >"$nextest_config"
# The baseline's tests have no time limit of the run's own, so load can't cut them off: run
# 43829665927's baseline was cut off at 30 minutes with 1754 of its 1998 tests passed, at a
# load of about 80 on 30 CPUs (2026-11-16), and a run whose baseline times out tests no
# mutant. A hung test still fails it: nextest ends each try at 5 minutes (above), and its
# failure stops the run. Whatever else could hang (a build, a lock) the shard's job limit
# ends, and the summary reads that shard as incomplete.
#
# Each mutant's tests are given 4 times the baseline's, and never less than 30 minutes. The
# baseline runs only the mutated packages' tests, and each mutant runs their dependents' too
# (.github/mutants-tests.py), so a mutant's tests can take longer than the baseline's: run
# 43796510701's took up to 965 s against its 280 s baseline (3.4 times, 2026-11-16). Load
# lengthens both alike, so at 3 times the widest of them would time out once load took the
# baseline past about 520 s; at 4 it never does. A package with many dependents and few tests of
# its own (silico-protocol, whose mutants run every workspace test) can still pass the
# limit: its small baseline leaves it at the 30-minute floor. cargo-mutants has no upper
# bound on the limit, and the worst case is a mutant that slows tests without failing them
# (one that fails a test ends the run there: the profile keeps nextest's fail-fast), which
# runs to the limit. On CI's hosted runners (mutants.yml) a shard holds 40 mutants in a job of
# 6 hours: a caught one takes about 2 minutes, so some 80 minutes of them, and its setup and
# baseline under half an hour, leave about 3.5 hours, room for six mutants that run to a
# 30-minute limit, or fewer at a longer one (a baseline past 450 s). A shard past them ends at
# its job's limit, and the summary reads it incomplete. (On the CI VM, loaded, at 80 mutants
# a shard of 24 hours, run 44253716210's took about 250 s each against a 1305 s baseline.) A
# fixed 30 minutes would instead time out every mutant once the baseline passed it, each a
# timeout triaged as missed.
#
# The workspace's tests take about 28 to 30 minutes on a hosted runner's 4 vCPUs (gate runs
# 45372353423 and 45382461311, 2026-11-24; 15.5 minutes at about 7 threads on the CI VM): a
# mutant only the last packages' tests catch took 26 to 30 minutes on the VM (#445), so CI's
# plan narrows each run's tests to the packages that can see its mutants where it can
# (.github/mutants-tests.py). (At 300 s, every test had passed so far in 10 of
# run 39630661896's 11 timeouts.) A hung test, retried twice on CI, takes 15 minutes from
# when it starts, at most about 5 into the run: nextest ends it first, and its failure
# catches the mutant. A run past its limit times out, and a timeout is triaged as missed.
cargo mutants --in-place --minimum-test-timeout 1800 --timeout-multiplier 4 \
  --cargo-test-arg=-E --cargo-test-arg="$filter" \
  --cargo-test-arg=--config-file --cargo-test-arg="$nextest_config" \
  --cargo-test-arg=--profile --cargo-test-arg=silico-mutants "$@"
