# Sourced by tests/gate.sh, tests/mutants.sh, and tests/proptest-explore.sh: checks out the
# committed tree (HEAD, or the commit SILICO_TREE_COMMIT names, so a script can test commits
# older than itself) into a worktree in the user cache, one per checkout, reused between
# runs and cleaned of everything untracked except build caches (target/, target-mutants/,
# target-filler/ and .pixi/), so runs after the first are incremental, and cds into it. (Not
# inside the repository: Cargo would take the outer checkout's workspace for the tree's.)
# SILICO_GATE_TREE names another place for it: CI's jobs on GitHub's hosted runners put it at
# one path in every job, so each restores the caches another saved (.github/actions/prepare).
# The gate, mutation testing, and the proptest exploration share it, taking turns under one
# lock; a mutants run keeps a build of its own in it (SILICO_TARGET, below), so neither
# evicts the other's.
#
# Re-runs the sourcing script ("$0", with its arguments) holding the lock, so source it
# before anything with side effects.

# shellcheck source=tests/lane-caches.sh
source "$(dirname "${BASH_SOURCE[0]}")/lane-caches.sh"

root=$(git rev-parse --show-toplevel)
commit=$(git -C "$root" rev-parse --verify "${SILICO_TREE_COMMIT:-HEAD}^{commit}")
checkout_id=$(printf '%s' "$root" | { sha256sum 2>/dev/null || shasum -a 256; } | cut -c1-16)
tree=${SILICO_GATE_TREE:-${XDG_CACHE_HOME:-$HOME/.cache}/silico/gate/$checkout_id}
mkdir -p "$(dirname "$tree")"

# Removes trees, made readable and writable first: a test killed before it cleans up can leave
# a directory it made read-only (the CLI's fmt-in-a-read-only-directory test, in target/tmp),
# whose entries rm can't unlink, or unreadable (d-wx-wx-wx), which rm can't list.
remove_tree() {
  chmod -R u+rwX "$@" 2>/dev/null || true
  rm -rf "$@"
}

# One run at a time per checkout: a second run would switch the shared worktree to its
# own commit halfway through the first. Re-run this script holding a lock on it (Perl's
# flock, as third_party/occt/scripts/cache.sh does; released if the process dies).
if [ "${SILICO_GATE_LOCKED-}" != 1 ]; then
  exec env SILICO_GATE_LOCKED=1 perl -MFcntl=:flock -e '
    BEGIN { $^F = 1023 }
    my $lock = shift;
    open(my $fh, ">>", $lock) or die "gate: can'\''t open $lock: $!\n";
    if (!flock($fh, LOCK_EX | LOCK_NB)) {
      print STDERR "gate: waiting for another gate or mutants run on this checkout ($lock)\n";
      flock($fh, LOCK_EX) or die "gate: can'\''t lock $lock: $!\n";
    }
    exec(@ARGV) or die "gate: can'\''t run bash: $!\n";
  ' "$tree.lock" bash "$0" "$@"
fi

if [ -z "${SILICO_TREE_COMMIT-}" ] && [ -n "$(git -C "$root" status --porcelain)" ]; then
  echo "gate: note: uncommitted changes aren't checked; checking $commit" >&2
fi

if [ -e "$tree/.git" ] && ! git -C "$tree" rev-parse --git-dir >/dev/null 2>&1; then
  # Its checkout is gone, cloned again at the same path (a CI runner's is, every job):
  # start the worktree again, keeping its build caches.
  keep=$(mktemp -d "$tree.keep.XXXXXX")
  for cache in target target-mutants target-filler .pixi; do
    if [ -e "$tree/$cache" ]; then mv "$tree/$cache" "$keep/"; fi
  done
  rm -rf "$tree"
  git -C "$root" worktree prune
  git -C "$root" worktree add --detach --quiet "$tree" "$commit"
  for cache in target target-mutants target-filler .pixi; do
    if [ -e "$keep/$cache" ]; then mv "$keep/$cache" "$tree/"; fi
  done
  rmdir "$keep"
fi
if [ ! -e "$tree/.git" ]; then
  # A worktree registered here but deleted: forget only that one, not the user's others.
  git -C "$root" worktree remove --force "$tree" 2>/dev/null || true
  git -C "$root" worktree add --detach --quiet "$tree" "$commit"
fi
# Caches a run moved aside to start the worktree again (above), and couldn't move back because
# it stopped first (`git worktree add` failing on a full disk): each put back where the tree
# has none, the rest let go, so none sits beside the tree for good.
for keep in "$tree".keep.*; do
  [ -d "$keep" ] || continue
  for cache in target target-mutants target-filler .pixi; do
    if [ -e "$keep/$cache" ] && [ ! -e "$tree/$cache" ]; then mv "$keep/$cache" "$tree/"; fi
  done
  remove_tree "$keep"
done
# The build's own target directory: target/, or the one SILICO_TARGET names (CI's mutants
# shards build without debug info in target-mutants/, so neither build evicts or invalidates
# the other; target-filler/ was the CI VM's filler's, kept until a week unbuilt, below).
# target/occt stays in target/: the build scripts find OCCT there.
target=$(lane_of_run)
# A lane's cache unbuilt for a week goes, here and in each sibling checkout no run holds
# (tests/lane-caches.sh): its runner no longer builds that lane there.
prune_for_run "$tree"
target_limit_gib=${SILICO_TARGET_LIMIT_GIB:-30}
# $target/debug keeps a copy of each crate for each set of dependencies and features it was
# built with (each branch's Cargo.lock, cargo-mutants' baselines of a few packages), and
# Cargo never removes them: past a bound, start it again, cold (target/occt and the other
# caches stay). One whole gate's build is about 12 GiB, or 14 GiB with incremental
# compilation, which CI turns off. A build without debug info is a fraction of that, and
# takes a smaller bound (SILICO_TARGET_LIMIT_GIB).
if [ -d "$tree/$target/debug" ]; then
  target_kib=$(du -sk "$tree/$target/debug" | cut -f1) || {
    echo "gate: can't measure $tree/$target/debug (above); delete it by hand" >&2
    exit 1
  }
  target_gib=$((target_kib / 1024 / 1024))
  if [ "$target_gib" -gt "$target_limit_gib" ]; then
    echo "gate: $target/debug is $target_gib GiB, past $target_limit_gib GiB: deleting it, so this build starts cold" >&2
    rm -rf "$tree/$target/debug"
  else
    echo "gate: $target/debug is $target_gib GiB: kept (deleted past $target_limit_gib GiB)" >&2
  fi
fi
git -C "$tree" checkout --detach --quiet --force "$commit"
git -C "$tree" clean -fdx --quiet -e /target/ -e /target-mutants/ -e /target-filler/ -e /.pixi/
cd "$tree"
if [ "$target" != target ]; then export CARGO_TARGET_DIR="$tree/$target"; fi
# The tests' scratch (Cargo's CARGO_TARGET_TMPDIR, target/tmp: documents, logs, workers'
# dumps) where SILICO_TEST_SCRATCH says, as CI puts it in memory (.github/scratch.sh,
# whose `end` puts the directory back); otherwise a directory, as Cargo makes it.
mkdir -p "$target"
if [ -n "${SILICO_TEST_SCRATCH-}" ]; then
  mkdir -p "$SILICO_TEST_SCRATCH"
  if [ -L "$target/tmp" ]; then rm "$target/tmp"; else remove_tree "$target/tmp"; fi
  ln -s "$SILICO_TEST_SCRATCH" "$target/tmp"
elif [ -L "$target/tmp" ]; then
  rm "$target/tmp"
fi
