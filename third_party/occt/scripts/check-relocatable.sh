#!/usr/bin/env bash
# Fails if an OCCT install can't be shared between checkouts: a library searches anything
# other than its own directory for its dependencies, or any file contains a path under a
# forbidden prefix (such as the build directory or a home directory, which OCCT bakes in
# through __FILE__). One pass over the install, however many prefixes.
#
# Each prefix is also forbidden in its physical form (symlinks resolved). A prefix with one
# component (/, /src, /tmp) is skipped: it can't identify a machine or checkout, and it
# matches OCCT's own text (its remapped occt/src/... paths, its default /tmp).
# Usage: check-relocatable.sh <install dir> <forbidden prefix>...
set -euo pipefail
# shellcheck disable=SC1091
. "$(dirname "${BASH_SOURCE[0]}")/platform.sh"
# Resolved, so grep searches the install itself when given a link to it.
dir=$(cd "$1" && pwd -P)
shift
prefixes=()
for given in "$@"; do
  for prefix in "$given" "$(cd "$given" 2>/dev/null && pwd -P)"; do
    prefix=${prefix%/}
    case "$prefix" in
      /*/*) ;;
      *) continue ;;
    esac
    case " ${prefixes[*]-} " in *" $prefix "*) ;; *) prefixes+=("$prefix") ;; esac
  done
done
if [ ${#prefixes[@]} -eq 0 ]; then
  echo "FAIL: no forbidden prefixes with at least two components given (got: $*)" >&2
  exit 1
fi
patterns=()
for prefix in "${prefixes[@]}"; do patterns+=(-e "$prefix"); done
libs=()
for f in "$dir"/lib/libTK*; do
  if [ -f "$f" ] && [ ! -L "$f" ]; then libs+=("$f"); fi
done
if [ ${#libs[@]} -eq 0 ]; then
  echo "FAIL: no libraries in $dir/lib" >&2
  exit 1
fi

bad=""
for f in "${libs[@]}"; do
  if ! rpaths=$(rpaths_of "$f" 2>/dev/null); then
    bad="$bad\n  $(basename "$f") can't be read as a library"
    continue
  fi
  for rp in $rpaths; do
    [ "$rp" = "$own_dir_rpath" ] || bad="$bad\n  $(basename "$f") searches $rp"
  done
done
# grep exits 1 when nothing matches, and 2 when it can't read something.
if hits=$(grep -rlaF "${patterns[@]}" -- "$dir"); then
  while IFS= read -r f; do
    for prefix in "${prefixes[@]}"; do
      if [ "$(grep -acF -- "$prefix" "$f" || true)" != 0 ]; then
        bad="$bad\n  ${f#"$dir"/} contains a path under $prefix"
      fi
    done
  done <<< "$hits"
elif [ $? -ne 1 ]; then
  bad="$bad\n  couldn't read all of $dir"
fi
if [ -n "$bad" ]; then
  printf "FAIL: install tied to one machine or checkout:%b\n" "$bad" >&2
  exit 1
fi
echo "OK: install is relocatable (${#libs[@]} libraries checked)"
