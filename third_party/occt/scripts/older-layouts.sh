#!/usr/bin/env bash
# Prints each older layout of Silico's OCCT build cache in <cache root>: a v<n> directory,
# other than <current>, that holds the cache's own files. Anything else there is left
# alone, since <cache root> may be a shared directory like ~/.cache.
# Usage: older-layouts.sh <cache root> <current layout, such as v1>
set -euo pipefail
for dir in "$1"/v*; do
  if [[ $(basename "$dir") =~ ^v[0-9]+$ ]] && [ "$(basename "$dir")" != "$2" ] \
    && { [ -d "$dir/.links" ] || [ -d "$dir/downloads" ]; }; then
    echo "$dir"
  fi
done
