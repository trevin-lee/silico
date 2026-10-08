#!/usr/bin/env bash
# The shared OCCT build cache. A cache holds:
#   <stamp>          symlink to the build for that stamp (a 64-character hex hash)
#   .occt-*          the build directories the symlinks point at
#   .used-<stamp>    touched whenever a checkout uses <stamp>, for pruning
#   .links/<id>      one per checkout: the path of its target/occt directory, so pruning
#                    never removes a build that a checkout still links to
#   .build-*         builds in progress
#   .lock            every command below holds an exclusive lock on it, so no two of them
#                    ever interleave. Builds run outside the lock; only these short steps
#                    are serialized. The kernel releases the lock if the holder dies.
#
# An entry is valid when it's a symlink to a build whose install/.stamp holds its stamp.
# Anything else (a build or stamp deleted by hand, a stamp-named real directory left by a
# copy that followed symlinks) is broken: publishing replaces it, using it fails, and
# pruning removes it. (Validity doesn't cover a stray library deleted from a valid build;
# the checks in build.sh only see the libraries that are there.)
#
# Usage:
#   cache.sh publish <cache> <stamp> <build dir>   move a complete build in as <stamp>,
#                                                  unless a valid one is there already
#   cache.sh use <cache> <stamp> <checkout dir>    if <stamp> is valid, point <checkout dir>/
#                                                  {install,src,downloads} at it, register the
#                                                  checkout, and record the use; else exit 1
#   cache.sh prune <cache> <keep>                  keep the <keep> most recently used valid
#                                                  entries, plus older ones a checkout links
#                                                  to; remove everything else
set -euo pipefail
# shellcheck disable=SC1091
. "$(dirname "${BASH_SOURCE[0]}")/platform.sh"

if [ "${1-}" != --locked ]; then
  # Re-run this script holding the lock. $^F: keep the lock's descriptor open across exec.
  # A wait is announced, so a lock held by a stopped process (Ctrl-Z) isn't a silent hang.
  cache=$(cd "$2" && pwd -P)
  exec perl -MFcntl=:flock -e '
    BEGIN { $^F = 1023 }
    my $lock = shift;
    open(my $fh, ">>", $lock) or die "cache.sh: can'\''t open $lock: $!\n";
    if (!flock($fh, LOCK_EX | LOCK_NB)) {
      print STDERR "occt: waiting for the build cache lock ($lock), held by another pixi run occt\n";
      flock($fh, LOCK_EX) or die "cache.sh: can'\''t lock $lock: $!\n";
    }
    exec(@ARGV) or die "cache.sh: can'\''t run bash: $!\n";
  ' "$cache/.lock" bash "${BASH_SOURCE[0]}" --locked "$@"
fi
shift

stamp_re='^[0-9a-f]{64}$'
cmd=$1
# Absolute and canonical, so links into the cache resolve from anywhere and registrations
# compare equal however the cache was named.
cache=$(cd "$2" && pwd -P)

valid() {
  [ -L "$cache/$1" ] && [ -f "$cache/$1/install/.stamp" ] && [ "$(cat "$cache/$1/install/.stamp")" = "$1" ]
}

# Remove an entry (a symlink, or a real directory in its place), its build, and its record.
remove_entry() {
  local target
  target=$(readlink "$cache/$1" || true)
  rm -rf "${cache:?}/$1"
  rm -f "${cache:?}/.used-$1"
  case "$target" in .occt-*) rm -rf "${cache:?}/$target" ;; esac
}

case "$cmd" in
  publish)
    stamp=$3
    work=$4
    if [ "$(cat "$work/install/.stamp" 2>/dev/null)" != "$stamp" ]; then
      echo "cache.sh: refusing to publish $work: its install/.stamp isn't $stamp" >&2
      exit 1
    fi
    if valid "$stamp"; then
      rm -rf "$work"
      echo "already published"
      exit 0
    fi
    result=published
    if [ -e "$cache/$stamp" ] || [ -L "$cache/$stamp" ]; then
      remove_entry "$stamp"
      result="published over a broken entry"
    fi
    built_dir=.occt-${stamp:0:16}-$(basename "$work" | sed 's/^\.build-//')
    mv "$work" "$cache/$built_dir"
    # -n: never follow an existing <stamp> into a directory and link inside it.
    ln -sn "$built_dir" "$cache/$stamp"
    touch "$cache/.used-$stamp"
    echo "$result"
    ;;
  use)
    stamp=$3
    valid "$stamp" || exit 1
    mkdir -p "$4" "$cache/.links"
    checkout=$(cd "$4" && pwd)
    ln -sfn "$cache/$stamp/install" "$checkout/install"
    ln -sfn "$cache/$stamp/src" "$checkout/src"
    ln -sfn "$cache/downloads" "$checkout/downloads"
    id=$(printf '%s' "$checkout" | sha256_of | cut -d' ' -f1)
    printf '%s\n' "$checkout" > "$cache/.links/$id"
    touch "$cache/.used-$stamp"
    ;;
  prune)
    keep=$3
    # Builds interrupted more than a day ago. A build in progress is younger than that.
    find "$cache" -maxdepth 1 -name '.build-*' -mmin +1440 -exec rm -rf {} + 2>/dev/null || true

    # Broken entries go first, so they never count toward <keep>. Valid entries with no
    # record of use (from before records existed) count as the oldest.
    # shellcheck disable=SC2012
    for stamp in $(ls "$cache" | grep -E "$stamp_re" || true); do
      if ! valid "$stamp"; then
        remove_entry "$stamp"
      elif [ ! -e "$cache/.used-$stamp" ]; then
        touch -t 200001010000 "$cache/.used-$stamp"
      fi
    done

    # Stamps a live checkout links to are never removed. A registration whose checkout
    # is gone, or now links elsewhere, is dropped.
    linked=""
    if [ -d "$cache/.links" ]; then
      for reg in "$cache"/.links/*; do
        [ -f "$reg" ] || continue
        target=$(readlink "$(cat "$reg")/install" 2>/dev/null || true)
        case "$target" in
          "$cache"/*/install) linked="$linked $(basename "$(dirname "$target")")" ;;
          *) rm -f "$reg" ;;
        esac
      done
    fi

    # Records are regular files, so ls -t sorts them by last use (it can't be relied on
    # to sort symlinks by their own times). -a: they're dotfiles. Linked entries count
    # toward <keep>, so at most <keep> unlinked entries survive.
    rank=0
    # shellcheck disable=SC2012
    for used in $(ls -at "$cache" | grep -E '^\.used-' || true); do
      stamp=${used#.used-}
      if ! [[ $stamp =~ $stamp_re ]] || ! valid "$stamp"; then
        # A record of an entry that's gone. (Publishing records a use under the lock,
        # so this is never an entry being published.)
        rm -f "${cache:?}/$used"
        continue
      fi
      rank=$((rank + 1))
      case " $linked " in *" $stamp "*) continue ;; esac
      [ "$rank" -le "$keep" ] && continue
      remove_entry "$stamp"
    done

    # Builds no entry points at, left when publishing was interrupted. Publishing moves a
    # build in and links it under the lock, so none of these is being published.
    live=" "
    for entry in "$cache"/*; do
      [[ $(basename "$entry") =~ $stamp_re ]] && [ -L "$entry" ] || continue
      live="$live$(readlink "$entry") "
    done
    for dir in "$cache"/.occt-*; do
      [ -d "$dir" ] || continue
      case "$live" in *" $(basename "$dir") "*) ;; *) rm -rf "$dir" ;; esac
    done
    ;;
  *)
    echo "cache.sh: unknown command $cmd" >&2
    exit 2
    ;;
esac
