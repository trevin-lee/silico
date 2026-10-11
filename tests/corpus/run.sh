#!/usr/bin/env bash
# The corpus of real parts (docs/roadmap/m1-kernel.md, M1.9): downloads each dataset once
# into the user cache (never committed), checks it against its pinned SHA-256, and runs
# `silico-oracle corpus` on its STEP files.
#
# Datasets, each license checked before use:
# - NIST's CAD models and STEP files with PMI (MBE PMI validation and conformance
#   testing): "can be used without any restrictions" (NIST's disclaimer). Its 33 part
#   files, AP242 and AP203.
#
# Run through pixi:  pixi run corpus
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cache=${XDG_CACHE_HOME:-$HOME/.cache}/silico/corpus

fetch() { # name url sha256
  local name=$1 url=$2 sha=$3 dir=$cache/$1
  if [ ! -e "$dir/.done" ]; then
    rm -rf "$dir"
    mkdir -p "$dir"
    echo "corpus: downloading $name" >&2
    curl -fsSL --retry 3 -o "$dir/archive.zip" "$url"
    local got
    got=$( { sha256sum "$dir/archive.zip" 2>/dev/null || shasum -a 256 "$dir/archive.zip"; } | cut -d' ' -f1)
    if [ "$got" != "$sha" ]; then
      echo "corpus: $name's archive has SHA-256 $got, not $sha" >&2
      rm -rf "$dir"
      exit 1
    fi
    unzip -q -j "$dir/archive.zip" '*.stp' -d "$dir"
    rm "$dir/archive.zip"
    touch "$dir/.done"
  fi
}

fetch nist \
  https://www.nist.gov/system/files/documents/noindex/2024/06/19/NIST-PMI-STEP-Files.zip \
  8fa78429e6d8d9b0d7681d223b6aa9ec98c3772185c55b1a0e3679b21c181911

cd "$root"
cargo build -q -p silico-kernel --bins
cargo run -q -p silico-oracle -- corpus "$cache/nist"
