#!/usr/bin/env bash
# Real work takes precedence over filler (.github/workflows/filler.yml): cancel every filler
# run, of the mutants workflow, the oracle sweep, or the proptest exploration, queued or
# running (each title starts "filler "). A real mutants run, the bench, and the nightly
# sweep call this first, from a runner labelled control, so filler shards can't keep them
# waiting. A cancelled filler shard still uploads what it finished. Needs GH_TOKEN (with
# actions: write) and GH_REPO. Never fails the run that calls it: real work goes on even if
# GitHub's API doesn't answer.
set -uo pipefail
. "${BASH_SOURCE[0]%/*}/gh.sh" || exit 0

for workflow in mutants.yml oracle-sweep.yml proptest-explore.yml; do
	gh run list --workflow "$workflow" --limit 100 --json databaseId,status,displayTitle \
		--jq '.[] | select(.status != "completed" and (.displayTitle | startswith("filler "))) | .databaseId' ||
		echo "::warning::couldn't list $workflow's filler runs" >&2
done | while read -r run; do
	echo "Cancelling filler run $run, for this real work"
	gh run cancel "$run" || echo "::warning::couldn't cancel filler run $run (it may have just ended)"
done
exit 0
