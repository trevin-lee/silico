#!/usr/bin/env bash
# tests/lane-caches.sh's prune_stale_lanes, run against made-up gate checkouts. Each lane's
# cache is a directory, and each stamp is made as old as the case needs (perl's utime, as
# BSD's and GNU's touch differ). Each case says what must stay and what must go. Needs bash,
# find and perl; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/lane-caches-test.sh)
set -euo pipefail

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
*"target/debug not built here for more than 7 days"*) ;;
*) fail "8 days: it said \"$said\", not why target/debug went" ;;
esac
pass

# Each lane its own: the same checkout from the gate (target/), the filler's cache eight days
# unbuilt goes and the mutants' doesn't, nor the gate's, whose stamp it renews.
tree=$(checkout gate)
prune_stale_lanes "$tree" target 7 2>/dev/null
age 8 "$tree/target-filler/.silico-lane-stamp" "$tree/target/.silico-lane-stamp"
prune_stale_lanes "$tree" target 7 2>/dev/null
[ ! -e "$tree/target-filler/debug" ] || fail "the gate's run: target-filler/debug wasn't deleted"
[ -d "$tree/target-mutants/debug" ] || fail "the gate's run: target-mutants/debug was deleted"
[ -d "$tree/target/debug" ] || fail "the gate's run: its own target/debug was deleted"
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
