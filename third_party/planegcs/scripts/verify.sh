#!/usr/bin/env bash
# Fails unless src/ is FreeCAD 1.1.3's PlaneGCS (pristine.sha256) with exactly the patches
# in series applied: each patch must reverse-apply in turn, last first, and what's left must
# be the release's files byte for byte, with nothing added. Offline: the checksums came from
# the release archive (VERSION).
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp -R "$here/src" "$tmp/src"
# Reverse order: a later patch may touch what an earlier one did. patch(1), not git apply,
# which skips patches inside another repository (third_party/occt/README.md).
awk '{ line[NR] = $0 } END { for (i = NR; i > 0; i--) print line[i] }' "$here/series" |
  while read -r p; do
    if ! patch -d "$tmp" -p1 -R -f --batch --silent < "$here/patches/$p" >/dev/null 2>&1; then
      echo "FAIL: patch $p is not applied to third_party/planegcs/src" >&2
      exit 1
    fi
  done
# sha256sum (Linux) and shasum -a 256 (macOS) print the same lines.
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
if ! (cd "$tmp" && find src -type f | LC_ALL=C sort | while read -r f; do sha256 "$f"; done | diff -u "$here/pristine.sha256" -) >&2; then
  echo "FAIL: third_party/planegcs/src isn't FreeCAD 1.1.3's PlaneGCS plus the series" >&2
  exit 1
fi
echo "OK: PlaneGCS is FreeCAD 1.1.3's, with $(wc -l < "$here/series" | tr -d ' ') patch(es)"
