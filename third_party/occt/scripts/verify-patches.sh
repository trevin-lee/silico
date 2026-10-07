#!/usr/bin/env bash
# Fails unless every patch in the series is present in <source dir>: each must
# reverse-apply cleanly. Uses --force: with --batch, patch "assumes" a reverse patch
# that looks already reversed was meant forwards, so the check would pass on
# unpatched source.
# Usage: verify-patches.sh <source dir>
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
src=$1
while read -r p; do
  if ! patch -d "$src" -p1 -R -f --dry-run --silent < "$here/patches/$p" >/dev/null 2>&1; then
    echo "FAIL: patch $p is not applied to $src" >&2
    exit 1
  fi
done < "$here/series"
echo "OK: all patches are applied"
