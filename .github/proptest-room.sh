#!/usr/bin/env bash
# Whether a scheduled proptest exploration (proptest-explore.yml) may start: filler's rules
# (filler.yml), for one run on one runner labelled filler. It writes go=true or go=false, and
# overdue=true or false (below), to GITHUB_OUTPUT, and why to the log and the summary. No,
# while
#   - a bench or an oracle sweep is going: a bench measures times, and a sweep, a night's or
#     filler's, holds cases to deadlines, which a run at the idle weight beside it would
#     share the CPU of;
#   - filler's oracle sweep is due (no filler sweep started on BRANCH in four hours, as
#     filler.yml reckons it) or overdue (none in a day but those cancelled): the filler waits
#     for an exploration going before it starts the sweep, so one started now would hold it
#     back by up to an hour;
#   - a real mutants run is starting (no shard yet: its plan is cancelling filler, and its
#     shards will want filler's runners) or has a shard waiting for a runner: unlike filler.yml,
#     which lets filler start beside a lane whose every runner is busy, this can't ask for
#     silico-22 or -23 alone, so it waits whenever real work does;
#   - a fifth of the VM's CPUs or fewer are idle, over 15 seconds (this runs on the VM): its
#     tests, starved, would time out and read as failures;
#   - every runner labelled filler is busy.
# Overdue, when no exploration's explore job has passed or failed, having run a round (its step
# "Rounds ran"), in a day, it goes whatever CPU is idle, and beside a sweep merely due: real
# mutants runs can keep the VM a fifth or less idle for days (2026-11-04 and 05), and a sweep
# due waits for idle CPU too. Not while a bench or a sweep is going, nor while filler's sweep
# is overdue too (no filler sweep in a day but those cancelled: it goes first, filler.yml, and
# starts none beside an exploration, so neither waits on the other for long), nor while a gate
# is waiting for a runner, a real mutants run is starting, or one has a shard waiting that a
# free runner of its lane could take (filler.yml's rule: one waiting behind its lane's busy
# runners waits whatever this does, and every runner labelled filler busy still holds this
# back, so its job never queues to race one for the next runner freed). Its tests then take a
# real shard's share of threads, 2, and its slow proptests run alone (proptest-explore.yml,
# tests/proptest-explore.sh).
# Once it starts, a bench or a night's sweep starting cancels it, as they cancel every filler
# run, and so does a real mutants run starting, unless it's overdue: then only if that run's
# shards would wait for a runner of their lane otherwise (cancel-filler.sh), so a busy VM,
# where real runs start several times a day, still gets its day's exploration. A failed look
# at GitHub's runs fails this, and with it the run: no exploration starts unchecked.
#
# Needs GH_TOKEN (actions: read), GH_REPO, and BRANCH, the branch filler runs on (main).
set -euo pipefail
. "${BASH_SOURCE[0]%/*}/gh.sh"
# shellcheck source=.github/lanes.sh
. "${BASH_SOURCE[0]%/*}/lanes.sh"

# The runners labelled filler (filler.yml's pool), how often filler sweeps (its
# oracle_every), and how long a sweep or an exploration may wait before it's overdue.
pool=10
oracle_every=$((4 * 3600))
day=$((24 * 3600))
overdue=false

report() { echo "$1"; echo "$1" >>"$GITHUB_STEP_SUMMARY"; }
decide() {
  report "$2"
  { echo "go=$1"; echo "overdue=$overdue"; } >>"$GITHUB_OUTPUT"
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

# Overdue: no explore job passed or failed in a day having run a round (explored,
# .github/lanes.sh, which a real mutants run's cancelling of filler asks too).
explored=$(explored "$day")
if [ "$explored" -eq 0 ]; then
  overdue=true
  report "Overdue: no exploration has passed or failed in a day."
fi

# The filler sweeps, as filler.yml counts them (sweeps, .github/lanes.sh): overdue (none in a
# day but those cancelled), whether or not one started in four hours (one cancelled may have),
# the sweep goes first; merely due, only an exploration not overdue waits for it.
recent=$(sweeps "$oracle_every")
today=$(sweeps "$day" uncancelled)
if [ "$today" -eq 0 ]; then
  if [ "$overdue" = true ]; then
    decide false "Not started: filler's oracle sweep is overdue too, and goes first."
  fi
  decide false "Not started: filler's oracle sweep is overdue, and goes first."
fi
if [ "$recent" -eq 0 ] && [ "$overdue" = false ]; then
  decide false "Not started: filler's oracle sweep is due, and goes first."
fi
if [ "$overdue" = true ] && awk -F '\t' '$2 == "gate.yml" && $3 == "queued" { found = 1 } END { exit !found }' <<<"$active"; then
  decide false "Not started: a gate is waiting for a runner."
fi

# The jobs of every run going: the run's id, the job's status, its runner, its name, and its
# labels.
jobs=$(awk -F '\t' 'NF { print $1 }' <<<"$active" | while read -r run; do
  gh api "repos/$GH_REPO/actions/runs/$run/jobs?per_page=100" --jq \
    ".jobs[] | [\"$run\", .status, (.runner_name // \"\"), .name, (.labels // [] | join(\",\"))] | @tsv" || exit 1
done)
# Runners running a job now, of any run going, by number (pve-silico-N-...), for free_for.
running=" $(awk -F '\t' '$2 == "in_progress" && match($3, /^pve-silico-[0-9]+-/) { print substr($3, 12, RLENGTH - 12) }' <<<"$jobs" | tr '\n' ' ')"
for run in $(awk -F '\t' '$2 == "mutants.yml" && $4 !~ /^filler / { print $1 }' <<<"$active"); do
  # A run with no shard job yet is starting (its plan, before the matrix). Its other jobs
  # (the plan, the summary) take the runners labelled control, which this doesn't.
  shards=$(awk -F '\t' -v run="$run" '$1 == run && $4 ~ /^shard/ { n++ } END { print n + 0 }' <<<"$jobs")
  waiting=$(awk -F '\t' -v run="$run" '$1 == run && $4 ~ /^shard/ && $2 != "in_progress" && $2 != "completed" { n++ } END { print n + 0 }' <<<"$jobs")
  if [ "$shards" -eq 0 ] || { [ "$waiting" -gt 0 ] && [ "$overdue" = false ]; }; then
    decide false "Not started: a real mutants run ($run) is starting or has a shard waiting for a runner."
  fi
  # Overdue (or none waiting), only a shard that a free runner could take holds it back.
  for label in $(awk -F '\t' -v run="$run" '$1 == run && $4 ~ /^shard/ && $2 != "in_progress" && $2 != "completed" { n = split($5, l, ","); for (i = 1; i <= n; i++) print l[i] }' <<<"$jobs" | sort -u); do
    if free_for "$label"; then
      decide false "Not started: a real mutants run ($run) has a shard waiting for $label, and a runner of that lane is free."
    fi
  done
done

# The runners labelled filler running a job now, of any run: silico-5 to -12, -22, and -23.
busy=$(awk -F '\t' '$2 == "in_progress" && $3 ~ /^pve-silico-([5-9]|1[0-2]|2[23])-/ { n++ } END { print n + 0 }' <<<"$jobs")
if [ "$busy" -ge "$pool" ]; then
  decide false "Not started: all $pool runners labelled filler are busy."
fi
if [ "$overdue" = true ]; then
  decide true "Room, overdue: $busy of $pool runners labelled filler busy, whatever CPU is idle."
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
