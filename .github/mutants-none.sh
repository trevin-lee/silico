#!/usr/bin/env bash
# Whether a mutants shard whose cargo-mutants ended well had no mutants to test, for the run's
# summary (mutants.yml). cargo-mutants 27.1.0 writes no list at all when its --in-diff filter
# leaves nothing (run 43831530760: "No mutants to filter", a change to data only), and an
# empty one ([]) when the other filters do (or sharding leaves a shard none); the summary
# reads a shard with no list as one stopped before it listed its mutants. So, once
# cargo-mutants has exited 0 (never for a shard that stopped or failed, which must still read
# as incomplete):
#
#   bash .github/mutants-none.sh MUTANTS_OUT COMMIT [FILE...]
#
# If MUTANTS_OUT holds no mutants.json, or one listing none, writes MUTANTS_OUT/none: empty if
# each FILE matches a file in COMMIT's tree, else a line for each FILE that matches none, so a
# misspelt file still fails the run rather than passing having tested nothing. A FILE is a
# --file glob, matched as cargo-mutants matches it (https://mutants.rs/skip_files.html): from
# the tree's root if it holds a /, else against each file's and directory's name, in any
# directory. Here `*` also crosses a /, and `**` between slashes (or leading) is also tried as
# no directory at all, as globset matches it: either only lets more paths match, never fewer.
set -euo pipefail

out=$1
commit=$2
shift 2
if [ -e "$out/mutants.json" ]; then
  # A list that can't be read is left to the summary, which says so.
  listed=$(python3 -I -c 'import json, sys; print(len(json.load(open(sys.argv[1]))))' "$out/mutants.json" 2>/dev/null) ||
    exit 0
  if [ "$listed" != 0 ]; then exit 0; fi
fi
paths=$(git ls-tree -r --name-only "$commit")
mkdir -p "$out"
unmatched=""
for file in "$@"; do
  # The glob, and the glob with each `**` taken as no directory.
  globs=("$file")
  if [[ $file == */* ]]; then
    bare=${file//\/\*\*\//\/}
    bare=${bare#\*\*/}
    if [ "$bare" != "$file" ]; then globs+=("$bare"); fi
  fi
  matched=""
  while IFS= read -r path; do
    if [[ $file == */* ]]; then
      candidates=("$path")
    else
      IFS=/ read -ra candidates <<<"$path"
    fi
    for candidate in "${candidates[@]}"; do
      for glob in "${globs[@]}"; do
        # shellcheck disable=SC2053 # $glob is a glob, matched as one
        if [[ $candidate == $glob ]]; then
          matched=1
          break 3
        fi
      done
    done
  done <<<"$paths"
  if [ -z "$matched" ]; then unmatched+="no file in the tree matches $file"$'\n'; fi
done
# Whole or not at all: a marker cut short by a failed write would drop a misspelt file's line.
printf '%s' "$unmatched" >"$out/none.tmp"
mv "$out/none.tmp" "$out/none"
