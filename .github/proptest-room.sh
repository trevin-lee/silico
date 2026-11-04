#!/usr/bin/env bash
# Whether a scheduled proptest exploration (proptest-explore.yml) may start: filler's rules
# (filler.yml), for one run on one runner labelled filler. It writes go=true or go=false to
# GITHUB_OUTPUT, and why to the log and the summary. No, while
#   - a bench or an oracle sweep is going: a bench measures times, and a sweep, a night's or
#     filler's, holds cases to deadlines, which a run at the idle weight beside it would
#     share the CPU of;
#   - a real mutants run is starting (no shard yet: its plan is cancelling filler, and its
#     shards will want filler's runners) or has a shard waiting for a runner: unlike filler.yml,
#     which lets filler start beside a lane whose every runner is busy, this can't ask for
#     silico-22 or -23 alone, so it waits whenever real work does;
#   - a fifth of the VM's CPUs or fewer are idle, over 15 seconds (this runs on the VM): its
#     tests, starved, would time out and read as failures;
#   - every runner labelled filler is busy.
# Once it starts, real work that starts cancels it, as it does every filler run
# (cancel-filler.sh). A failed look at GitHub's runs fails this, and with it the run: no
# exploration starts unchecked.
#
# Needs GH_TOKEN (actions: read) and GH_REPO.
set -euo pipefail
. "${BASH_SOURCE[0]%/*}/gh.sh"

# The runners labelled filler (filler.yml's pool).
pool=10

report() { echo "$1"; echo "$1" >>"$GITHUB_STEP_SUMMARY"; }
decide() {
  report "$2"
  echo "go=$1" >>"$GITHUB_OUTPUT"
  exit 0
}

# Every queued or running run but this one: its id, workflow file, status, title.
active=$(for status in queued in_progress; do
  gh api "repos/$GH_REPO/actions/runs?status=$status&per_page=100" --jq \
    ".workflow_runs[] | select(.id != $GITHUB_RUN_ID)
      | [.id, (.path | ltrimstr(\".github/workflows/\")), .status, .display_title] | @tsv" || exit 1
done)
if awk -F '\t' '$2 == "bench.yml" || $2 == "oracle-sweep.yml" { found = 1 } END { exit !found }' <<<"$active"; then
  decide false "Not started: a bench or an oracle sweep is going."
fi

# The jobs of every run going: the run's id, the job's status, its runner, and its name.
jobs=$(awk -F '\t' 'NF { print $1 }' <<<"$active" | while read -r run; do
  gh api "repos/$GH_REPO/actions/runs/$run/jobs?per_page=100" --jq \
    ".jobs[] | [\"$run\", .status, (.runner_name // \"\"), .name] | @tsv" || exit 1
done)
for run in $(awk -F '\t' '$2 == "mutants.yml" && $4 !~ /^filler / { print $1 }' <<<"$active"); do
  # A run with no shard job yet is starting (its plan, before the matrix). Its other jobs
  # (the plan, the summary) take the runners labelled control, which this doesn't.
  shards=$(awk -F '\t' -v run="$run" '$1 == run && $4 ~ /^shard/ { n++ } END { print n + 0 }' <<<"$jobs")
  waiting=$(awk -F '\t' -v run="$run" '$1 == run && $4 ~ /^shard/ && $2 != "in_progress" && $2 != "completed" { n++ } END { print n + 0 }' <<<"$jobs")
  if [ "$shards" -eq 0 ] || [ "$waiting" -gt 0 ]; then
    decide false "Not started: a real mutants run ($run) is starting or has a shard waiting for a runner."
  fi
done

# The runners labelled filler running a job now, of any run: silico-5 to -12, -22, and -23.
busy=$(awk -F '\t' '$2 == "in_progress" && $3 ~ /^pve-silico-([5-9]|1[0-2]|2[23])-/ { n++ } END { print n + 0 }' <<<"$jobs")
if [ "$busy" -ge "$pool" ]; then
  decide false "Not started: all $pool runners labelled filler are busy."
fi

# The CPUs idle now, as filler.yml reads them: idle and I/O wait over all time spent.
cpu() { awk '/^cpu / { print $2 + $3 + $4 + $5 + $6 + $7 + $8 + $9, $5 + $6 }' /proc/stat; }
read -r total0 idle0 < <(cpu)
sleep 15
read -r total1 idle1 < <(cpu)
cpus=$(nproc)
idle=$((cpus * (idle1 - idle0) / (total1 - total0 > 0 ? total1 - total0 : 1)))
if [ $((idle * 5)) -le "$cpus" ]; then
  decide false "Not started: $idle of $cpus CPUs are idle, a fifth or less."
fi
decide true "Room: $busy of $pool runners labelled filler busy, $idle of $cpus CPUs idle."
