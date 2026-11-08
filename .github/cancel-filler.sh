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
# overdue is cancelled only if the shards would wait for a runner it holds or would take. Their
# lane's spare runners are those free once the filler mutants are cancelled, less the jobs
# already waiting for its label (another real run's shards), which take them first. One
# running on a runner of the lane is kept if the spare runners are as many as the shards; one
# waiting for a runner, or not yet asking, if they're more, since it was waiting first and
# takes one of them; one running on a runner outside the lane (silico-22 or -23, or -5 to -8
# for a long run) whatever they are, since cancelling it frees none the shards could take (as
# filler.yml starts filler there beside shards waiting). On a busy VM real runs start several
# times a day, and cancelled by each, the minimum would never be met (2026-11-07, the first
# exploration after #396, run 42029601964). Every caller runs this from a runner labelled
# control, so filler shards can't keep it waiting. A cancelled filler shard still uploads what
# it finished.
# Needs GH_TOKEN (with actions: write) and GH_REPO. Never fails the run that calls it: real
# work goes on even if GitHub's API doesn't answer. Real work comes first, so an overdue run
# whose need it can't tell, for a failed look, is cancelled as before, with a warning.
set -uo pipefail
. "${BASH_SOURCE[0]%/*}/gh.sh" || exit 0
# shellcheck source=.github/lanes.sh
. "${BASH_SOURCE[0]%/*}/lanes.sh"

# Cancel run $1, saying why ($2) if given. Fails if GitHub didn't take the cancel.
cancel() {
  echo "Cancelling filler run $1${2:+: $2}"
  gh run cancel "$1" && return
  echo "::warning::couldn't cancel filler run $1 (it may have just ended)"
  return 1
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
# a later turn (filler.yml). Only those GitHub took the cancel of free their runners. (Each
# loop over the runs reads them from fd 3, so nothing it runs can take the rest from its
# standard input.)
cancelled=' '
uncancelled=false
while read -r run workflow _ <&3; do
  if [ "$workflow" = mutants.yml ]; then
    if cancel "$run"; then cancelled="$cancelled$run "; else uncancelled=true; fi
  fi
done 3<<<"$filler"

# The shards' lane's spare runners: those free, less the jobs waiting for its label, which
# take them first. Free, a runner running no job of a run going (as proptest-room.sh reads
# them), each running one in held (a run's id, its runner's number), but those of the filler
# mutants just cancelled; waiting, as filler.yml reads them, each unfinished job not yet
# running that asks for the label (but the filler mutants'). spare is a number, or, for a
# failed look at GitHub or a filler mutants run not cancelled (whose runners may not come
# free), unknown. Asked once, and only for a run overdue.
spare=
held=
spare_runners() {
  local active jobs run running free waiting
  spare=unknown
  if [ "$uncancelled" = true ]; then
    echo "A filler mutants run wasn't cancelled: its runners may not come free"
    return
  fi
  active=$(for status in queued in_progress; do
    gh api "repos/$GH_REPO/actions/runs?status=$status&per_page=100" --jq '.workflow_runs[].id' || exit 1
  done) || return
  # Each job's run, status, runner, and labels.
  jobs=$(for run in $active; do
    if [[ $cancelled == *" $run "* ]]; then continue; fi
    gh api "repos/$GH_REPO/actions/runs/$run/jobs?per_page=100" --jq \
      ".jobs[] | [\"$run\", .status, (.runner_name // \"\"), (.labels // [] | join(\",\"))] | @tsv" || exit 1
  done) || return
  held=$(awk -F '\t' '$2 == "in_progress" && match($3, /^pve-silico-[0-9]+-/) { print $1 "\t" substr($3, 12, RLENGTH - 12) }' <<<"$jobs")
  running=" $(awk -F '\t' '{ print $2 }' <<<"$held" | tr '\n' ' ')"
  free=$(free_count "$label")
  waiting=$(awk -F '\t' -v label="$label" '$2 != "in_progress" && $2 != "completed" {
      n = split($4, l, ","); for (i = 1; i <= n; i++) if (l[i] == label) { count++; break } }
    END { print count + 0 }' <<<"$jobs")
  spare=$((free - waiting))
  echo "$free runners labelled $label free once filler mutants are cancelled, $waiting jobs waiting for one: $spare spare for $shards shards"
}

# Where run $1 is: on a runner of the shards' lane (lane), on one labelled filler the lane
# hasn't (elsewhere), where cancelling it would free none they could take, or neither
# (waiting): queued for a runner, or not yet asking for one, it takes the next one freed,
# perhaps theirs.
place() {
  local n where=waiting
  for n in $(awk -F '\t' -v run="$1" '$1 == run { print $2 }' <<<"$held"); do
    if in_lane "$label" "$n"; then
      echo lane
      return
    fi
    if in_lane filler "$n"; then where=elsewhere; fi
  done
  echo "$where"
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
  [ -n "$spare" ] || spare_runners
  if [ "$spare" = unknown ]; then
    echo "::warning::couldn't tell whether the shards need the overdue $what $run's runner: cancelled, as real work comes first"
    cancel "$run"
    continue
  fi
  case $(place "$run") in
  elsewhere) echo "Keeping the $what $run, overdue: it's on a runner the shards' lane hasn't" ;;
  lane)
    if [ "$spare" -ge "$shards" ] 2>/dev/null; then
      echo "Keeping the $what $run, overdue: the lane has a runner spare for each shard"
    else
      cancel "$run" "the $what is overdue, but the shards would wait for its runner"
    fi
    ;;
  *)
    if [ "$spare" -gt "$shards" ] 2>/dev/null; then
      echo "Keeping the $what $run, overdue: the lane has a runner spare for it and each shard"
      spare=$((spare - 1))
    else
      cancel "$run" "the $what is overdue and waiting, and would take a runner the shards wait for"
    fi
    ;;
  esac
done 3<<<"$filler"
exit 0
