#!/usr/bin/env bash
# Whether a mutants shard whose cargo-mutants ended well found no mutants at all, for the
# run's summary (mutants.yml). cargo-mutants 27.1.0 writes nothing when it finds no mutants
# under its filters, not even an empty mutants.json (run 43831530760: "No mutants to filter",
# a change to data only), and the summary reads a shard with no list as one stopped before it
# listed its mutants. So, once cargo-mutants has exited 0 (never for a shard that stopped or
# failed, which must still read as incomplete):
#
#   bash .github/mutants-none.sh MUTANTS_OUT COMMIT [FILE...]
#
# If MUTANTS_OUT holds no mutants.json, writes MUTANTS_OUT/none: empty if each FILE matches a
# file in COMMIT's tree, else a line for each FILE that matches none, so a misspelt file still
# fails the run rather than passing having tested nothing. A FILE is a --file glob, matched as
# cargo-mutants matches it (https://mutants.rs/skip_files.html): from the tree's root if it
# holds a /, else against each file's and directory's name, in any directory (here `*` also
# crosses a /, which only lets more paths match).
set -euo pipefail

out=$1
commit=$2
shift 2
if [ -e "$out/mutants.json" ]; then exit 0; fi
paths=$(git ls-tree -r --name-only "$commit")
mkdir -p "$out"
unmatched=""
for file in "$@"; do
  matched=""
  while IFS= read -r path; do
    if [[ $file == */* ]]; then
      candidates=("$path")
    else
      IFS=/ read -ra candidates <<<"$path"
    fi
    for candidate in "${candidates[@]}"; do
      # shellcheck disable=SC2053 # $file is a glob, matched as one
      if [[ $candidate == $file ]]; then
        matched=1
        break 2
      fi
    done
  done <<<"$paths"
  if [ -z "$matched" ]; then unmatched+="no file in the tree matches $file"$'\n'; fi
done
printf '%s' "$unmatched" >"$out/none"
