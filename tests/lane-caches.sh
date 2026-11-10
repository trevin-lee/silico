# Sourced by tests/committed-tree.sh, and by its test, tests/lane-caches-test.sh. A gate
# checkout keeps a build cache for each lane that builds in it:
#   - target/, the gate's;
#   - target-mutants/, a mutants shard's;
#   - target-filler/, the filler's.
# Each lane's cache is stamped when that lane builds there. A lane's cache that hasn't been
# built in a checkout for more than a set number of days is let go: a runner whose labels
# changed, or that was retired or relabelled to take none of these lanes, would otherwise keep
# it whole, since a lane's own size bound runs only when that lane builds. Only its debug/ goes:
# target/occt, which the build scripts find in target/, stays.

# This file, by its absolute path, for a sibling's prune to source again under that sibling's
# lock (prune_sibling_checkouts).
lane_caches_lib=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")

# The lane a run of committed-tree.sh builds for: target/, or the one SILICO_TARGET names.
lane_of_run() {
  echo "${SILICO_TARGET:-target}"
}

# prune_lane TREE LANE DAYS: delete LANE's debug/ in the checkout TREE if its stamp is more than
# DAYS days old; a stamp not there yet is made, so a cache is counted from when it's first seen.
# Nothing for a lane with no debug/.
prune_lane() {
  local tree=$1 lane=$2 days=$3 stamp
  [ -d "$tree/$lane/debug" ] || return 0
  stamp=$tree/$lane/.silico-lane-stamp
  if [ ! -e "$stamp" ]; then
    touch "$stamp"
  elif [ -n "$(find "$stamp" -mmin +$((days * 1440)))" ]; then
    echo "gate: $tree/$lane/debug not built for more than $days days: deleting it" >&2
    # Writable first: a test killed mid-run can leave a directory it made read-only.
    chmod -R u+rwX "$tree/$lane/debug" 2>/dev/null || true
    rm -rf "$tree/$lane/debug"
  fi
}

# prune_stale_lanes TREE TARGET DAYS: stamp TARGET's cache in the checkout TREE (so this run's
# own lane is never past DAYS), then let go each lane's debug/ there unbuilt for more than DAYS
# days. Runs under committed-tree.sh's lock on TREE, as every build in it does.
prune_stale_lanes() {
  local tree=$1 target=$2 days=$3 lane
  mkdir -p "$tree/$target"
  touch "$tree/$target/.silico-lane-stamp"
  for lane in target target-mutants target-filler; do
    prune_lane "$tree" "$lane" "$days"
  done
}

# prune_idle_checkout TREE DAYS: each lane's debug/ in the checkout TREE unbuilt for more than
# DAYS days let go, none stamped as this run's: a sibling checkout's.
prune_idle_checkout() {
  local tree=$1 days=$2 lane
  for lane in target target-mutants target-filler; do
    prune_lane "$tree" "$lane" "$days"
  done
}

# prune_sibling_checkouts TREE DAYS: the same for every other checkout beside TREE that
# committed-tree.sh made (it has a lock file), taking its lock without waiting and skipping it
# if a run holds it. That way a runner that no longer builds in its checkout, relabelled to
# none of these lanes or retired, still has its caches let go. The lock is held, across the
# exec, while its caches are looked at, as committed-tree.sh holds its own.
prune_sibling_checkouts() {
  local tree=$1 days=$2 sibling
  for sibling in "$(dirname "$tree")"/*/; do
    sibling=${sibling%/}
    [ "$sibling" != "$tree" ] && [ -e "$sibling.lock" ] || continue
    perl -MFcntl=:flock -e '
      BEGIN { $^F = 1023 }
      my $lock = shift;
      open(my $fh, ">>", $lock) or exit 0;
      flock($fh, LOCK_EX | LOCK_NB) or exit 0;
      exec(@ARGV) or exit 0;
    ' "$sibling.lock" bash -c 'source "$1" && prune_idle_checkout "$2" "$3"' _ "$lane_caches_lib" "$sibling" "$days" || true
  done
}

# prune_for_run TREE: what committed-tree.sh does for a run in the checkout TREE: its own lane
# (lane_of_run) stamped, and every lane unbuilt for more than SILICO_STALE_LANE_DAYS days (7)
# let go, in TREE and in each sibling checkout no run holds.
prune_for_run() {
  local tree=$1 days=${SILICO_STALE_LANE_DAYS:-7}
  prune_stale_lanes "$tree" "$(lane_of_run)" "$days"
  prune_sibling_checkouts "$tree" "$days"
}
