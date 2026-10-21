#!/usr/bin/env bash
# A nightly's result as a GitHub issue (oracle-sweep.yml): a failed night opens an issue
# titled for its workflow, or comments on the one already open; a green night closes it
# with a comment. Run by the workflow's last job, on scheduled runs, with the job's token
# (GH_TOKEN, GH_REPO), the night's result (RESULT: success, failure, cancelled), WORKFLOW,
# RUN_URL, and DETAIL (what to say about the night, such as its seed).
set -euo pipefail

GH_VERSION=2.102.1
GH_SHA256=77b9e297e94c50336823d0f84de7ef1be60cba704601fa0d98f129f47a94987b

# The runners are single-use: gh, verified, if the image hasn't it.
if ! command -v gh >/dev/null 2>&1; then
  dir=$(mktemp -d)
  curl -fsSL -o "$dir/gh.tar.gz" \
    "https://github.com/cli/cli/releases/download/v$GH_VERSION/gh_${GH_VERSION}_linux_amd64.tar.gz"
  echo "$GH_SHA256  $dir/gh.tar.gz" | sha256sum -c --quiet
  tar zxf "$dir/gh.tar.gz" -C "$dir" --strip-components 2 "gh_${GH_VERSION}_linux_amd64/bin/gh"
  PATH="$dir:$PATH"
fi

title="Nightly $WORKFLOW failed"
# The open issue with exactly this title, if any (the search matches words, not the title).
open=$(gh issue list --state open --search "in:title $title" --json number,title \
  --jq ".[] | select(.title == \"$title\") | .number" | head -n 1)

case $RESULT in
success)
  if [ -n "$open" ]; then
    gh issue comment "$open" --body "Green again: $RUN_URL"
    gh issue close "$open"
    echo "closed #$open"
  fi
  ;;
skipped)
  # Nothing ran (a skipped job says nothing about the code).
  ;;
*)
  body="The nightly $WORKFLOW ended $RESULT: $RUN_URL${DETAIL:+

$DETAIL}"
  if [ -n "$open" ]; then
    gh issue comment "$open" --body "$body"
    echo "commented on #$open"
  else
    gh issue create --title "$title" --body "$body"
  fi
  ;;
esac
