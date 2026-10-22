# Sourced by tests/gate.sh and tests/mutants.sh: checks out the committed tree (HEAD, or
# the commit SILICO_TREE_COMMIT names, so a script can test commits older than itself) into
# a worktree in the user cache, one per checkout, reused between runs and cleaned of
# everything untracked except build caches (target/ and .pixi/), so runs after the first are
# incremental, and cds into it. (Not inside the repository: Cargo would take the outer
# checkout's workspace for the tree's.) The gate and mutation testing share it, so each
# starts from the other's build; they take turns, under one lock.
#
# Re-runs the sourcing script ("$0", with its arguments) holding the lock, so source it
# before anything with side effects.

root=$(git rev-parse --show-toplevel)
commit=$(git -C "$root" rev-parse --verify "${SILICO_TREE_COMMIT:-HEAD}^{commit}")
checkout_id=$(printf '%s' "$root" | { sha256sum 2>/dev/null || shasum -a 256; } | cut -c1-16)
tree=${XDG_CACHE_HOME:-$HOME/.cache}/silico/gate/$checkout_id
mkdir -p "$(dirname "$tree")"

# Removes trees, made writable first: a test can leave a directory it made read-only (the
# CLI's read-only save tests, cache_bound's), whose entries rm can't unlink.
remove_tree() {
  chmod -R u+w "$@" 2>/dev/null || true
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
  for cache in target .pixi; do
    if [ -e "$tree/$cache" ]; then mv "$tree/$cache" "$keep/"; fi
  done
  remove_tree "$tree"
  git -C "$root" worktree prune
  git -C "$root" worktree add --detach --quiet "$tree" "$commit"
  for cache in target .pixi; do
    if [ -e "$keep/$cache" ]; then mv "$keep/$cache" "$tree/"; fi
  done
  rmdir "$keep"
fi
if [ ! -e "$tree/.git" ]; then
  # A worktree registered here but deleted: forget only that one, not the user's others.
  git -C "$root" worktree remove --force "$tree" 2>/dev/null || true
  git -C "$root" worktree add --detach --quiet "$tree" "$commit"
fi
# target/debug keeps a copy of each crate for each set of dependencies and features it was
# built with (each branch's Cargo.lock, cargo-mutants' baselines of a few packages), and
# Cargo never removes them: past a bound, start it again, cold (target/occt and the other
# caches stay). One whole gate's build is about 12 GiB, or 14 GiB with incremental
# compilation, which CI turns off.
target_limit_gib=30
if [ -d "$tree/target/debug" ]; then
  target_kib=$(du -sk "$tree/target/debug" | cut -f1) || {
    echo "gate: can't measure $tree/target/debug (above); delete it by hand" >&2
    exit 1
  }
  target_gib=$((target_kib / 1024 / 1024))
  if [ "$target_gib" -gt "$target_limit_gib" ]; then
    echo "gate: target/debug is $target_gib GiB, past $target_limit_gib GiB: deleting it, so this build starts cold" >&2
    remove_tree "$tree/target/debug"
  else
    echo "gate: target/debug is $target_gib GiB: kept (deleted past $target_limit_gib GiB)" >&2
  fi
fi
git -C "$tree" checkout --detach --quiet --force "$commit"
git -C "$tree" clean -fdx --quiet -e /target/ -e /.pixi/
cd "$tree"
# The tests' scratch (Cargo's CARGO_TARGET_TMPDIR, target/tmp: documents, logs, workers'
# dumps) where SILICO_TEST_SCRATCH says, as CI puts it in memory (.github/scratch.sh,
# whose `end` puts the directory back); otherwise a directory, as Cargo makes it.
mkdir -p target
if [ -n "${SILICO_TEST_SCRATCH-}" ]; then
  mkdir -p "$SILICO_TEST_SCRATCH"
  remove_tree target/tmp
  ln -s "$SILICO_TEST_SCRATCH" target/tmp
elif [ -L target/tmp ]; then
  rm target/tmp
fi
