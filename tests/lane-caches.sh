# Sourced by tests/committed-tree.sh, and by its test, tests/lane-caches-test.sh. A gate
# checkout keeps a build cache for each lane that builds in it:
#   - target/, the gate's;
#   - target-mutants/, a mutants shard's;
#   - target-filler/, the filler's.
# Each lane's cache is stamped when that lane builds there. Another lane's is let go once it
# has gone unbuilt in this checkout for more than a set number of days: when a runner's labels
# change, it stops taking a lane, and that lane's cache would otherwise sit there whole. Each
# lane's own size bound never reaches it, since that bound runs only when its lane builds.
# Only its debug/ goes: target/occt, which the build scripts find in target/, stays.

# prune_stale_lanes TREE TARGET DAYS: stamp TARGET's cache in the checkout TREE, then delete
# each lane's debug/ whose stamp is more than DAYS days old. A stamp not there yet is
# made, so a cache is counted from when it's first seen. Runs under committed-tree.sh's lock,
# as every build in the checkout does, so no build uses what it deletes.
prune_stale_lanes() {
  local tree=$1 target=$2 days=$3 lane stamp
  mkdir -p "$tree/$target"
  # Stamped now, so this run's own lane is never past DAYS below.
  touch "$tree/$target/.silico-lane-stamp"
  for lane in target target-mutants target-filler; do
    [ -d "$tree/$lane/debug" ] || continue
    stamp=$tree/$lane/.silico-lane-stamp
    if [ ! -e "$stamp" ]; then
      touch "$stamp"
    elif [ -n "$(find "$stamp" -mtime +"$days")" ]; then
      echo "gate: $lane/debug not built here for more than $days days (its lane runs elsewhere now): deleting it" >&2
      # Writable first: a test killed mid-run can leave a directory it made read-only.
      chmod -R u+rwX "$tree/$lane/debug" 2>/dev/null || true
      rm -rf "$tree/$lane/debug"
    fi
  done
}
