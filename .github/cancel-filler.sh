#!/usr/bin/env bash
# Real work takes precedence over filler (.github/workflows/filler.yml). With no arguments, as
# the bench and the nightly sweep call it, it cancels every filler run, of the mutants
# workflow, the oracle sweep, or the proptest exploration, queued or running (each title
# starts "filler "): a bench measures times and a night's sweep holds cases to deadlines, so
# both want the CPU quiet. Given workflow files, only their filler runs: an oracle sweep
# overdue cancels filler mutants alone (filler.yml).
# A real mutants run's plan calls it with the label its shards ask for and how many there are
# (--lane LABEL --shards N, mutants.yml). It cancels every filler mutants run, and every
# exploration and filler sweep but one overdue, whose once-a-day minimum is owed (explored and
# sweeps, .github/lanes.sh: the rules the exploration's room and the filler start them by). One
# overdue is cancelled only if the shards would wait for a runner it holds or would take: the
# runners of their lane free once the filler mutants are cancelled are fewer than the shards,
# and it isn't on a runner outside that lane (silico-22 or -23, or -5 to -8 for a long run),
# where cancelling it frees none they could take (as filler.yml starts filler there beside
# shards waiting). On a busy VM real runs start several times a day, and cancelled by each,
# the minimum would never be met (2026-11-07, the first exploration after #396, run
# 42029601964). Every caller runs this from a runner labelled control, so filler shards can't
# keep it waiting. A cancelled filler shard still uploads what it finished.
# Needs GH_TOKEN (with actions: write) and GH_REPO. Never fails the run that calls it: real
# work goes on even if GitHub's API doesn't answer. Real work comes first, so an overdue run
# whose need it can't tell, for a failed look, is cancelled as before, with a warning.
set -uo pipefail
. "${BASH_SOURCE[0]%/*}/gh.sh" || exit 0
# shellcheck source=.github/lanes.sh
. "${BASH_SOURCE[0]%/*}/lanes.sh"

# Cancel run $1, saying why ($2) if given.
cancel() {
  echo "Cancelling filler run $1${2:+: $2}"
  gh run cancel "$1" || echo "::warning::couldn't cancel filler run $1 (it may have just ended)"
}

# The filler runs going, queued or running, of each workflow file given: each one's id, its
# workflow file, and its branch.
going() {
  local workflow
  for workflow in "$@"; do
    gh run list --workflow "$workflow" --limit 100 --json databaseId,status,displayTitle,headBranch \
      --jq ".[] | select(.status != \"completed\" and (.displayTitle | startswith(\"filler \")))
        | [.databaseId, \"$workflow\", .headBranch] | @tsv" ||
      echo "::warning::couldn't list $workflow's filler runs" >&2
  done
}

if [ "${1-}" != --lane ]; then
  if [ $# -eq 0 ]; then set -- mutants.yml oracle-sweep.yml proptest-explore.yml; fi
  going "$@" | while read -r run _; do
    cancel "$run"
  done
  exit 0
fi

# A real mutants run's: the label its shards ask for, and how many there are.
label=${2-}
shards=${4-}
day=$((24 * 3600))
filler=$(going mutants.yml oracle-sweep.yml proptest-explore.yml)

# Every filler mutants run, whatever is free: it fills idle time, and comes back to its work on
# a later turn (filler.yml). (Each loop over the runs reads them from fd 3, so nothing it runs
# can take the rest from its standard input.)
cancelled=' '
while read -r run workflow _ <&3; do
  if [ "$workflow" = mutants.yml ]; then
    cancel "$run"
    cancelled="$cancelled$run "
  fi
done 3<<<"$filler"

# Whether the shards' lane is short of runners for them: its runners free are fewer than the
# shards. Busy, every runner running a job of a run going (as proptest-room.sh reads them),
# each in held (a run's id, its runner's number), but those of the filler mutants just
# cancelled. short is true, false, or, for a failed look at GitHub, unknown. Asked once, and
# only for a run overdue.
short=
held=
lane_short() {
  local active run running free
  short=unknown
  active=$(for status in queued in_progress; do
    gh api "repos/$GH_REPO/actions/runs?status=$status&per_page=100" --jq '.workflow_runs[].id' || exit 1
  done) || return
  held=$(for run in $active; do
    if [[ $cancelled == *" $run "* ]]; then continue; fi
    gh api "repos/$GH_REPO/actions/runs/$run/jobs?per_page=100" --jq \
      ".jobs[] | select(.status == \"in_progress\") | \"$run\t\" + (.runner_name // \"\")" || exit 1
  done) || return
  held=$(awk -F '\t' 'match($2, /^pve-silico-[0-9]+-/) { print $1 "\t" substr($2, 12, RLENGTH - 12) }' <<<"$held")
  running=" $(awk -F '\t' '{ print $2 }' <<<"$held" | tr '\n' ' ')"
  free=$(free_count "$label")
  echo "$free runners labelled $label free once filler mutants are cancelled, for $shards shards"
  if [ "$free" -ge "$shards" ] 2>/dev/null; then short=false; else short=true; fi
}

# Whether run $1 holds a runner labelled filler that the shards' lane hasn't: cancelling it
# would free none they could take. One queued, or not yet on filler's runners, would take the
# next one freed, perhaps theirs.
elsewhere() {
  local n
  for n in $(awk -F '\t' -v run="$1" '$1 == run { print $2 }' <<<"$held"); do
    if in_lane filler "$n" && ! in_lane "$label" "$n"; then return 0; fi
  done
  return 1
}

# The runs of workflow $1 on BRANCH that met its once-a-day minimum in the last day. None, and
# one going is overdue.
met() {
  case $1 in
  oracle-sweep.yml) sweeps "$day" uncancelled ended ;;
  proptest-explore.yml) explored "$day" ;;
  esac
}

# Each exploration and filler sweep: cancelled unless overdue, then only if the shards would
# wait for its runner. Overdue as its own branch's runs say: the branch the exploration's room,
# or the filler that started the sweep, reckoned it on.
while read -r run workflow branch <&3; do
  case $workflow in
  oracle-sweep.yml) what='filler sweep' ;;
  proptest-explore.yml) what=exploration ;;
  *) continue ;;
  esac
  BRANCH=$branch
  if ! runs_met=$(met "$workflow"); then
    echo "::warning::couldn't tell whether the $what $run is overdue: cancelled, as real work comes first"
    cancel "$run"
    continue
  fi
  if [ "$runs_met" != 0 ]; then
    cancel "$run" "the $what isn't overdue"
    continue
  fi
  [ -n "$short" ] || lane_short
  case $short in
  false) echo "Keeping the $what $run, overdue: the lane has a runner free for each shard" ;;
  true)
    if elsewhere "$run"; then
      echo "Keeping the $what $run, overdue: it's on a runner the shards' lane hasn't"
    else
      cancel "$run" "the $what is overdue, but the shards would wait for its runner"
    fi
    ;;
  *)
    echo "::warning::couldn't tell whether the shards need the overdue $what $run's runner: cancelled, as real work comes first"
    cancel "$run"
    ;;
  esac
done 3<<<"$filler"
exit 0
