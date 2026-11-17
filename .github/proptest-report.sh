#!/usr/bin/env bash
# A proptest exploration's result as GitHub issues (proptest-explore.yml), as
# nightly-report.sh reports a night's: each issue tests/proptest-failures.py wrote (at most
# five of a property that failed again alone, "Proptest exploration failed: <binary> <test>",
# and one listing every other failed test, "... unconfirmed") is opened, assigned to the
# repository's owner, or commented on if one of its title is open. Nothing closes those: a
# green run's seeds tested other cases. A run that failed with no test to name (the build, or
# nextest itself) opens or comments on "Proptest exploration failed: setup", which the next
# green run closes, since it tests the same thing every run.
#
# A run that failed in its rounds but named no test reports nothing: its step's time limit
# stopped it mid-round (its outcome file says "rounds"), which is no failure of anything.
#
# Run by the workflow's last job, on scheduled runs, with the job's token (GH_TOKEN, GH_REPO,
# OWNER), the exploration's RESULT (success or failure), RUN_URL, and DIR, the exploration's
# output (its failures/, if any test failed, and its outcome, tests/proptest-explore.sh),
# as the VM keeps it or, failing that, its artifact (missing if it wrote none).
set -euo pipefail

# gh, verified.
. "${BASH_SOURCE[0]%/*}/gh.sh"

# Every open issue's number and title, from the issue list itself (not the search index,
# which lags and can be down: a missed lookup would open a duplicate).
open_issues=$(gh issue list --state open --limit 1000 --json number,title \
  --jq '.[] | "\(.number)\t\(.title)"')
open_for() {
  awk -F '\t' -v t="$1" '$2 == t { print $1; exit }' <<<"$open_issues"
}
# An issue for title, its body in a file: a comment on the open one, or a new one.
raise() {
  local title=$1 body=$2 number
  number=$(open_for "$title")
  if [ -n "$number" ]; then
    gh issue comment "$number" --body-file "$body"
    echo "$title: commented on #$number"
  else
    gh issue create --title "$title" --assignee "$OWNER" --body-file "$body"
  fi
}

setup="Proptest exploration failed: setup"
reported=0
if [ -d "$DIR/failures" ]; then
  for failure in "$DIR"/failures/*/; do
    [ -f "$failure/title" ] || continue
    raise "$(head -n 1 "$failure/title")" "$failure/body.md"
    reported=$((reported + 1))
  done
fi

if [ "$RESULT" = success ]; then
  number=$(open_for "$setup")
  if [ -n "$number" ]; then
    gh issue comment "$number" --body "Green again: $RUN_URL"
    gh issue close "$number"
    echo "setup: closed #$number"
  fi
elif [ "$reported" -eq 0 ] && [ "$(cat "$DIR/outcome" 2>/dev/null)" = rounds ]; then
  echo "Stopped mid-round from outside (its step's time limit): nothing to report"
elif [ "$reported" -eq 0 ]; then
  body=$(mktemp)
  trap 'rm -f "$body"' EXIT
  printf 'The proptest exploration ended %s with no failed test to name (the build, or nextest itself): %s\n' \
    "$RESULT" "$RUN_URL" >"$body"
  raise "$setup" "$body"
fi
