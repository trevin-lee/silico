#!/usr/bin/env bash
# The gate (docs/roadmap/m0-foundations.md, M0.3): every check, on the committed tree,
# before a merge. Uncommitted changes aren't checked, so a branch can't pass because of a
# file that only exists in someone's working tree.
#
# The committed tree is checked out into a worktree in the user cache, one per checkout,
# reused between runs and cleaned of everything untracked except build caches (target/ and
# .pixi/), so runs after the first are incremental. (Not inside the repository: Cargo
# would take the outer checkout's workspace for the gate's.)
#
# Run through pixi:  pixi run gate
set -euo pipefail

root=$(git rev-parse --show-toplevel)
commit=$(git -C "$root" rev-parse HEAD)
checkout_id=$(printf '%s' "$root" | { sha256sum 2>/dev/null || shasum -a 256; } | cut -c1-16)
tree=${XDG_CACHE_HOME:-$HOME/.cache}/silico/gate/$checkout_id
mkdir -p "$(dirname "$tree")"

# One run at a time per checkout: a second run would switch the shared worktree to its
# own commit halfway through the first. Re-run this script holding a lock on it (Perl's
# flock, as third_party/occt/scripts/cache.sh does; released if the process dies).
if [ "${SILICO_GATE_LOCKED-}" != 1 ]; then
  exec env SILICO_GATE_LOCKED=1 perl -MFcntl=:flock -e '
    BEGIN { $^F = 1023 }
    my $lock = shift;
    open(my $fh, ">>", $lock) or die "gate: can'\''t open $lock: $!\n";
    if (!flock($fh, LOCK_EX | LOCK_NB)) {
      print STDERR "gate: waiting for another gate run on this checkout ($lock)\n";
      flock($fh, LOCK_EX) or die "gate: can'\''t lock $lock: $!\n";
    }
    exec(@ARGV) or die "gate: can'\''t run bash: $!\n";
  ' "$tree.lock" bash "${BASH_SOURCE[0]}" "$@"
fi

if [ -n "$(git -C "$root" status --porcelain)" ]; then
  echo "gate: note: uncommitted changes aren't checked; checking $commit" >&2
fi

if [ -e "$tree/.git" ] && ! git -C "$tree" rev-parse --git-dir >/dev/null 2>&1; then
  # Its checkout is gone, cloned again at the same path (a CI runner's is, every job):
  # start the worktree again, keeping its build caches.
  keep=$(mktemp -d "$tree.keep.XXXXXX")
  for cache in target .pixi; do
    if [ -e "$tree/$cache" ]; then mv "$tree/$cache" "$keep/"; fi
  done
  rm -rf "$tree"
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
git -C "$tree" checkout --detach --quiet --force "$commit"
git -C "$tree" clean -fdx --quiet -e /target/ -e /.pixi/
cd "$tree"

step() { echo "gate: $*"; }

step "every crate uses the workspace lints"
for manifest in crates/*/Cargo.toml; do
  # [lints] followed by workspace = true, nothing between them.
  if ! awk '/^\[lints\]$/ { getline; if ($0 == "workspace = true") found = 1 } END { exit !found }' "$manifest"; then
    echo "gate: FAIL: $manifest doesn't have [lints] workspace = true" >&2
    exit 1
  fi
done

step "dependencies: banned crates, licenses, sources"
pixi run --quiet cargo deny --log-level error check bans licenses sources

step "lint"
pixi run lint

step "test"
pixi run test

step "determinism: one stress run"
STRESS_RUNS=1 pixi run stress

echo "gate: passed ($commit)"
