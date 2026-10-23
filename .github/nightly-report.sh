#!/usr/bin/env bash
# A nightly's result as GitHub issues (oracle-sweep.yml), one for each part that can fail:
# `sweep` (500 generated cases, a new seed each night), `corpus` (the same real parts each
# night), and `setup` (a failure before either ran). A failed part opens an issue titled
# "Nightly <workflow> failed: <part>", assigned to the repository's owner, or comments on
# the one already open. A green night closes the corpus's and setup's: they test the same
# thing every night. It only comments on the sweep's, which a green night can't settle (its
# seed tested other cases): it's closed by hand once the failing seed passes again.
#
# Run by the workflow's last job, on scheduled runs, with the job's token (GH_TOKEN,
# GH_REPO, OWNER), the night's RESULT (success, failure, cancelled, skipped), each part's
# step outcome (SWEEP, CORPUS: success, failure, cancelled, skipped, or empty if it never
# ran), WORKFLOW, RUN_URL, and SEED.
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
  setup) echo "It failed before the sweep or the corpus ran (the checkout or the toolchains)." ;;
  esac
}

failed() {
  local part=$1 outcome=$2 number body
  number=$(open_for "$part")
  body="The nightly $WORKFLOW's $part ended $outcome: $RUN_URL

$(how_to_repeat "$part")"
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
  if [ "$part" = sweep ]; then
    gh issue comment "$number" --body "Seed $SEED is green ($RUN_URL). Each night's seed tests other cases: close this once the failing seed passes again."
    echo "sweep: commented on #$number"
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
for part in sweep corpus; do
  case $part in
  sweep) outcome=${SWEEP:-} ;;
  corpus) outcome=${CORPUS:-} ;;
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
