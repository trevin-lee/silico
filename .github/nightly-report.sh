#!/usr/bin/env bash
# A nightly's result as GitHub issues (oracle-sweep.yml), one for each part that can fail:
# `sweep` (500 generated cases, a new seed each night), `corpus` (the same real parts each
# night), `sketch-edit-sweep` and `sketch-extend-sweep` (500 generated sketch edits and
# extends, from the night's seed), and `setup` (a failure before any ran). A failed part
# opens an issue titled "Nightly <workflow> failed: <part>", assigned to the repository's
# owner, or comments on the one already open; a sketch sweep's lists the cases it found
# differing. A green night closes the corpus's and setup's: they test the same thing every
# night. It only comments on the sweeps', which a green night can't settle (its seed tested
# other cases): each is closed by hand once its failing seed passes again.
#
# Run by the workflow's last job, on scheduled runs, with the job's token (GH_TOKEN,
# GH_REPO, OWNER), the night's RESULT (success, failure, cancelled, skipped), each part's
# step outcome (SWEEP, CORPUS, EDITS, EXTENDS: success, failure, cancelled, skipped, or
# empty if it never ran), the cases each sketch sweep found differing (EDIT_CASES,
# EXTEND_CASES: space-separated names, or empty), each sketch sweep's exit status
# (EDIT_STATUS, EXTEND_STATUS: 1 when cases differ, 2 when the sweep couldn't run, or empty),
# WORKFLOW, RUN_URL, and SEED.
set -euo pipefail

# gh, verified.
. "${BASH_SOURCE[0]%/*}/gh.sh"

# Every open issue's number and title, from the issue list itself (not the search index,
# which lags and can be down: a missed lookup would open a duplicate).
open_issues=$(gh issue list --state open --limit 1000 --json number,title \
  --jq '.[] | "\(.number)\t\(.title)"')

title_of() { echo "Nightly $WORKFLOW failed: $1"; }
open_for() {
  local title
  title=$(title_of "$1")
  awk -F '\t' -v t="$title" '$2 == t { print $1; exit }' <<<"$open_issues"
}

how_to_repeat() {
  case $1 in
  sweep) echo "Seed $SEED: \`pixi run oracle-sweep -- --seed $SEED\` repeats it." ;;
  corpus) echo "\`pixi run corpus\` repeats it." ;;
  sketch-edit-sweep) echo "Seed $SEED: \`pixi run oracle-sketch-edit-sweep -- --seed $SEED\` repeats it." ;;
  sketch-extend-sweep) echo "Seed $SEED: \`pixi run oracle-sketch-extend-sweep -- --seed $SEED\` repeats it." ;;
  setup) echo "It failed before any sweep or the corpus ran (the checkout or the toolchains)." ;;
  esac
}

# The cases a sketch sweep found differing, as a line of the issue, if it names any; or that
# it couldn't run at all (the oracle's exit 2: its own error, not a difference).
differing() {
  local cases status
  case $1 in
  sketch-edit-sweep) cases=${EDIT_CASES:-} status=${EDIT_STATUS:-} ;;
  sketch-extend-sweep) cases=${EXTEND_CASES:-} status=${EXTEND_STATUS:-} ;;
  *) cases= status= ;;
  esac
  if [ "$status" = 2 ]; then
    printf '\n\nThe sweep couldn'"'"'t run (the oracle exited 2): see its log.'
    return 0
  fi
  [ -n "$cases" ] || return 0
  printf '\n\nThe cases that differ: %s.' "$(sed 's/ /, /g' <<<"$cases")"
}

failed() {
  local part=$1 outcome=$2 number body
  number=$(open_for "$part")
  body="The nightly $WORKFLOW's $part ended $outcome: $RUN_URL

$(how_to_repeat "$part")$(differing "$part")"
  if [ -n "$number" ]; then
    gh issue comment "$number" --body "$body"
    echo "$part: commented on #$number"
  else
    gh issue create --title "$(title_of "$part")" --assignee "$OWNER" --body "$body"
  fi
}

green() {
  local part=$1 number
  number=$(open_for "$part")
  [ -n "$number" ] || return 0
  if [ "$part" != corpus ] && [ "$part" != setup ]; then
    gh issue comment "$number" --body "Seed $SEED is green ($RUN_URL). Each night's seed tests other cases: close this once the failing seed passes again."
    echo "$part: commented on #$number"
  else
    gh issue comment "$number" --body "Green again: $RUN_URL"
    gh issue close "$number"
    echo "$part: closed #$number"
  fi
}

case $RESULT in
skipped)
  # Nothing ran (a skipped job says nothing about the code).
  exit 0
  ;;
esac

any_part_failed=false
for part in sweep corpus sketch-edit-sweep sketch-extend-sweep; do
  case $part in
  sweep) outcome=${SWEEP:-} ;;
  corpus) outcome=${CORPUS:-} ;;
  sketch-edit-sweep) outcome=${EDITS:-} ;;
  sketch-extend-sweep) outcome=${EXTENDS:-} ;;
  esac
  case $outcome in
  success) green "$part" ;;
  failure | cancelled)
    failed "$part" "$outcome"
    any_part_failed=true
    ;;
  esac
done

if [ "$RESULT" = success ]; then
  green setup
elif [ "$any_part_failed" = false ]; then
  failed setup "$RESULT"
fi
