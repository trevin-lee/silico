#!/usr/bin/env bash
# tests/lane-caches.sh's prune_stale_lanes, run against made-up gate checkouts. Each lane's
# cache is a directory, and each stamp is made as old as the case needs (perl's utime, as
# BSD's and GNU's touch differ). Each case says what must stay and what must go. Needs bash,
# find and perl; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/lane-caches-test.sh)
set -euo pipefail

# The run's lane and the stale bound are the cases' to give, never the caller's: a lane
# exported for a mutants run (SILICO_TARGET) failed the default lane's case.
unset SILICO_TARGET SILICO_STALE_LANE_DAYS

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tests/lane-caches.sh
source "$repo/tests/lane-caches.sh"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/silico-lane-caches.XXXXXX")
trap 'chmod -R u+rwX "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

fail() {
  echo "lane-caches: $*" >&2
  exit 1
}

# age DAYS FILE...: each FILE's times made DAYS days old.
age() {
  perl -e '$t = time - shift(@ARGV) * 86400; utime($t, $t, @ARGV) == @ARGV or die "utime: $!\n"' "$@"
}

# A checkout named $1, each lane's cache built in it: a file in each debug/, and target/occt.
checkout() {
  local tree=$tmp/$1 lane
  for lane in target target-mutants target-filler; do
    mkdir -p "$tree/$lane/debug/deps"
    : >"$tree/$lane/debug/deps/libsilico_core-0123.rlib"
  done
  mkdir -p "$tree/target/occt/install"
  echo "$tree"
}

passed=0
pass() {
  passed=$((passed + 1))
}

# First seen: every cache is stamped, this run's lane's and the others', and none is deleted:
# a stamp starts counting when it's made.
tree=$(checkout first)
prune_stale_lanes "$tree" target-mutants 7 2>/dev/null
for lane in target target-mutants target-filler; do
  [ -e "$tree/$lane/.silico-lane-stamp" ] || fail "first seen: $lane wasn't stamped"
  [ -e "$tree/$lane/debug/deps/libsilico_core-0123.rlib" ] || fail "first seen: $lane/debug was deleted"
done
pass

# A week and a day unbuilt: the gate's cache goes on a mutants runner, its debug/ alone, with
# target/occt kept, and saying why. Six days unbuilt, the filler's stays. This run's own lane
# stays however old its stamp, and is stamped now.
tree=$(checkout week)
prune_stale_lanes "$tree" target-mutants 7 2>/dev/null
age 8 "$tree/target/.silico-lane-stamp"
age 6 "$tree/target-filler/.silico-lane-stamp"
age 30 "$tree/target-mutants/.silico-lane-stamp"
said=$(prune_stale_lanes "$tree" target-mutants 7 2>&1)
[ ! -e "$tree/target/debug" ] || fail "8 days: target/debug wasn't deleted"
[ -d "$tree/target/occt/install" ] || fail "8 days: target/occt went with target/debug"
[ -e "$tree/target-filler/debug/deps/libsilico_core-0123.rlib" ] || fail "6 days: target-filler/debug was deleted"
[ -e "$tree/target-mutants/debug/deps/libsilico_core-0123.rlib" ] || fail "its own lane: target-mutants/debug was deleted"
[ -z "$(find "$tree/target-mutants/.silico-lane-stamp" -mtime +0)" ] || fail "its own lane: its stamp wasn't renewed"
case $said in
*"$tree/target/debug not built for more than 7 days"*) ;;
*) fail "8 days: it said \"$said\", not why target/debug went" ;;
esac
pass

# Each lane its own: the same checkout from the gate (target/), the filler's cache eight days
# unbuilt goes and the mutants' doesn't, nor the gate's, whose stamp it renews.
tree=$(checkout gate)
prune_stale_lanes "$tree" target 7 2>/dev/null
[ -e "$tree/target-mutants/.silico-lane-stamp" ] || fail "the gate's run: target-mutants wasn't stamped"
age 8 "$tree/target-filler/.silico-lane-stamp" "$tree/target/.silico-lane-stamp"
prune_stale_lanes "$tree" target 7 2>/dev/null
[ ! -e "$tree/target-filler/debug" ] || fail "the gate's run: target-filler/debug wasn't deleted"
[ -d "$tree/target-mutants/debug" ] || fail "the gate's run: target-mutants/debug was deleted"
[ -d "$tree/target/debug" ] || fail "the gate's run: its own target/debug was deleted"
pass

# A gate runner's mutants cache, left by labels it no longer has: eight days unbuilt, it goes on
# the gate's run, target/occt kept.
tree=$(checkout relabelled)
prune_stale_lanes "$tree" target 7 2>/dev/null
age 8 "$tree/target-mutants/.silico-lane-stamp"
prune_stale_lanes "$tree" target 7 2>/dev/null
[ ! -e "$tree/target-mutants/debug" ] || fail "relabelled: target-mutants/debug wasn't deleted"
[ -d "$tree/target/occt/install" ] && [ -d "$tree/target/debug" ] || fail "relabelled: the gate's own cache went"
pass

# The bound is exact: seven days and an hour unbuilt goes, six days and 23 hours stays (find's
# -mtime would round both down to whole days).
tree=$(checkout exact)
prune_stale_lanes "$tree" target 7 2>/dev/null
age 7.0417 "$tree/target-mutants/.silico-lane-stamp"
age 6.9583 "$tree/target-filler/.silico-lane-stamp"
prune_stale_lanes "$tree" target 7 2>/dev/null
[ ! -e "$tree/target-mutants/debug" ] || fail "7 days and an hour: target-mutants/debug wasn't deleted"
[ -d "$tree/target-filler/debug" ] || fail "6 days and 23 hours: target-filler/debug was deleted"
pass

# What committed-tree.sh calls: prune_for_run takes the run's lane from SILICO_TARGET, so a
# mutants run renews its own cache, however old, and lets the gate's eight days unbuilt go.
tree=$(checkout run)
SILICO_TARGET=target-mutants prune_for_run "$tree" 2>/dev/null
age 30 "$tree/target-mutants/.silico-lane-stamp"
age 8 "$tree/target/.silico-lane-stamp"
SILICO_TARGET=target-mutants prune_for_run "$tree" 2>/dev/null
[ -d "$tree/target-mutants/debug" ] || fail "prune_for_run: the run's own target-mutants/debug went"
[ ! -e "$tree/target/debug" ] || fail "prune_for_run: target/debug eight days unbuilt stayed"
[ "$(SILICO_TARGET= lane_of_run)" = target ] && [ "$(lane_of_run)" = target ] || fail "lane_of_run: not target/ by default"
pass

# Sibling checkouts beside the run's, each with its lock file as committed-tree.sh makes them:
# an idle one's caches eight days unbuilt go; one whose lock a run holds is left as it is, and
# so is a directory with no lock file (not a checkout committed-tree.sh made).
mkdir -p "$tmp/gate"
run_tree=$(cd "$(checkout gate/mine)" && pwd)
idle=$(checkout gate/idle) busy=$(checkout gate/busy) other=$(checkout gate/other)
: >"$idle.lock"
: >"$busy.lock"
for sibling in "$idle" "$busy" "$other"; do
  prune_idle_checkout "$sibling" 7 2>/dev/null
  age 8 "$sibling"/target*/.silico-lane-stamp
done
perl -MFcntl=:flock -e 'open(my $fh, ">>", shift) or die; flock($fh, LOCK_EX) or die; $| = 1; print "held\n"; sleep 60' "$busy.lock" >"$tmp/held" &
holder=$!
for _ in $(seq 100); do grep -q held "$tmp/held" 2>/dev/null && break; sleep 0.1; done
grep -q held "$tmp/held" || fail "siblings: the busy checkout's lock was never held"
prune_for_run "$run_tree" 2>/dev/null
kill "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true
for lane in target target-mutants target-filler; do
  [ ! -e "$idle/$lane/debug" ] || fail "siblings: the idle checkout's $lane/debug stayed"
  [ -d "$busy/$lane/debug" ] || fail "siblings: the busy checkout's $lane/debug went"
  [ -d "$other/$lane/debug" ] || fail "siblings: the unlocked directory's $lane/debug went"
done
[ -d "$idle/target/occt/install" ] || fail "siblings: the idle checkout's target/occt went"
[ -d "$run_tree/target/debug" ] || fail "siblings: the run's own checkout lost its cache"
pass

# A lane with no debug/ (never built here, or already let go): nothing done to it, no stamp
# made; this run's lane is made and stamped.
tree=$tmp/bare
mkdir -p "$tree/target/occt"
prune_stale_lanes "$tree" target-filler 7 2>/dev/null
[ ! -e "$tree/target/.silico-lane-stamp" ] || fail "no debug/: target was stamped"
[ -d "$tree/target/occt" ] || fail "no debug/: target/occt went"
[ -e "$tree/target-filler/.silico-lane-stamp" ] || fail "no debug/: its own lane wasn't stamped"
pass

# A read-only directory a killed test left in a stale debug/ goes too (made writable first).
tree=$(checkout readonly)
prune_stale_lanes "$tree" target 7 2>/dev/null
mkdir -p "$tree/target-filler/debug/tmp/locked"
: >"$tree/target-filler/debug/tmp/locked/document.silico"
chmod 500 "$tree/target-filler/debug/tmp/locked"
age 8 "$tree/target-filler/.silico-lane-stamp"
prune_stale_lanes "$tree" target 7 2>/dev/null
[ ! -e "$tree/target-filler/debug" ] || fail "read-only: target-filler/debug wasn't deleted"
pass

echo "lane-caches: $passed cases passed"
