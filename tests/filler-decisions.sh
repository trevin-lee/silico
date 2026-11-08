#!/usr/bin/env bash
# The filler's decisions (.github/workflows/filler.yml's Fill step, .github/proptest-room.sh),
# and what real work cancels of it (.github/cancel-filler.sh), run against made-up states of
# the CI: GitHub's runs and jobs, served by a stub `gh` that applies each call's --jq to them
# with jq; the VM's CPUs (nproc, and /proc/stat, read from a file the stub `sleep` advances, as
# 15 seconds would). Each case says what the decision must start, cancel, or answer, and why,
# as it reports it. And a failed look at GitHub fails each, starting nothing, but for
# cancel-filler.sh, which never fails real work: it cancels what it can't tell is needed.
# Needs bash, jq, and awk; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/filler-decisions.sh)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/silico-filler-decisions.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

# The scripts as CI runs them, from a checkout of .github: the Fill step's script, cut from
# filler.yml, and proptest-room.sh, each reading the fake /proc/stat; and cancel-filler.sh.
mkdir -p "$tmp/checkout/.github" "$tmp/bin"
cp "$repo/.github/gh.sh" "$repo/.github/lanes.sh" "$repo/.github/cancel-filler.sh" "$tmp/checkout/.github/"
awk '/^      - name: Fill$/ { step = 1 } step && /^        run: \|$/ { body = 1; next } body { sub(/^          /, ""); print }' \
  "$repo/.github/workflows/filler.yml" | sed "s#/proc/stat#$tmp/stat#" >"$tmp/checkout/.github/fill.sh"
grep -q 'skip()' "$tmp/checkout/.github/fill.sh" || { echo "couldn't cut the Fill step from filler.yml" >&2; exit 1; }
sed "s#/proc/stat#$tmp/stat#" "$repo/.github/proptest-room.sh" >"$tmp/checkout/.github/proptest-room.sh"

# The stub gh. Runs come from runs.jsonl (id, path, status, conclusion, display_title,
# created_at; each on main), jobs from jobs.jsonl (run, name, status, runner_name, labels, conclusion,
# completed_at, steps); what it would change on GitHub goes to calls. A call whose arguments
# hold the text in the file fail fails, as GitHub not answering would.
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
filter=. args=()
while [ $# -gt 0 ]; do
  case $1 in
  --jq) filter=$2; shift 2 ;;
  *) args+=("$1"); shift ;;
  esac
done
set -- "${args[@]}"
if [ -s "$STATE/fail" ] && [[ "$*" == *"$(cat "$STATE/fail")"* ]]; then
  echo "stub gh: failing $*" >&2
  exit 1
fi
runs() { jq -s . "$STATE/runs.jsonl" 2>/dev/null || echo '[]'; }
case "$1 $2" in
"workflow run" | "run cancel") echo "$*" >>"$STATE/calls"; exit 0 ;;
"run list")
  workflow=
  while [ $# -gt 0 ]; do
    if [ "$1" = --workflow ]; then workflow=$2; fi
    shift
  done
  json=$(runs | jq --arg path ".github/workflows/$workflow" '[.[] | select(.path == $path)] | sort_by(.created_at) | reverse
    | map({databaseId: .id, status, conclusion: (.conclusion // ""), displayTitle: .display_title, createdAt: .created_at,
      headBranch: "main"})') ;;
api*)
  case $2 in
  */actions/runs\?status=*)
    status=${2#*status=}
    json=$(runs | jq --arg status "${status%%&*}" '{workflow_runs: [.[] | select(.status == $status)]}') ;;
  */actions/runs/*/jobs*)
    run=${2#*/actions/runs/}
    json=$(jq -s --argjson run "${run%%/*}" '{jobs: [.[] | select(.run == $run)]}' "$STATE/jobs.jsonl" 2>/dev/null || echo '{"jobs": []}') ;;
  */commits/main) json='{"sha": "1111111111111111111111111111111111111111"}' ;;
  *) echo "stub gh: no answer for api $2" >&2; exit 1 ;;
  esac ;;
*) echo "stub gh: no answer for $*" >&2; exit 1 ;;
esac
# gh's jq (gojq) takes Python's (?P<name>...) for a named group, jq's Oniguruma only (?<name>...).
jq -r "${filter//(?P</(?<}" <<<"$json"
EOF
printf '#!/bin/sh\necho 30\n' >"$tmp/bin/nproc"
printf '#!/bin/sh\ncp "$STATE/stat-after" "%s/stat"\n' "$tmp" >"$tmp/bin/sleep"
chmod +x "$tmp/bin/gh" "$tmp/bin/nproc" "$tmp/bin/sleep"

# The time $1 hours ago, as GitHub writes it.
ago() {
  local t=$(($(date +%s) - $1 * 3600))
  date -u -r "$t" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d "@$t" +%Y-%m-%dT%H:%M:%SZ
}
# A new state: the clock running, and $1 of the VM's 30 CPUs idle over the 15 seconds.
state() {
  STATE=$tmp/state
  rm -rf "$STATE"
  mkdir "$STATE"
  : >"$STATE/runs.jsonl"
  : >"$STATE/jobs.jsonl"
  : >"$STATE/calls"
  : >"$STATE/summary"
  : >"$STATE/output"
  echo "cpu  0 0 0 0 0 0 0 0" >"$tmp/stat"
  echo "cpu  $((3000 - 100 * $1)) 0 0 $((100 * $1)) 0 0 0 0" >"$STATE/stat-after"
  run 9000 filler-clock.yml in_progress filler-clock 0
}
# A run: its id, workflow file, status, title, how many hours ago it was made, and, ended, its
# conclusion.
run() {
  jq -nc --argjson id "$1" --arg path ".github/workflows/$2" --arg status "$3" --arg title "$4" --arg at "$(ago "$5")" \
    --arg conclusion "${6-}" \
    '{id: $id, path: $path, status: $status, conclusion: (if $conclusion == "" then null else $conclusion end),
      display_title: $title, created_at: $at}' >>"$STATE/runs.jsonl"
}
# A job: its run, name, status, runner, labels, and, ended, its conclusion and how many hours
# ago, and its step "Rounds ran"'s conclusion (none, empty).
job() {
  local at=null
  if [ -n "${7-}" ]; then at="\"$(ago "$7")\""; fi
  jq -nc --argjson run "$1" --arg name "$2" --arg status "$3" --arg runner "$4" --arg labels "$5" \
    --arg conclusion "${6-}" --argjson at "$at" --arg rounds "${8-}" \
    '{run: $run, name: $name, status: $status, runner_name: (if $runner == "" then null else $runner end),
      labels: ($labels | split(",")), conclusion: (if $conclusion == "" then null else $conclusion end),
      completed_at: $at,
      steps: (if $rounds == "" then [] else [{name: "Rounds ran", status: "completed", conclusion: $rounds}] end)}' \
    >>"$STATE/jobs.jsonl"
}
# A filler sweep made $1 hours ago, ended with conclusion $2 (success unless given).
swept() { run $((100 + $1)) oracle-sweep.yml completed "filler oracle sweep, seed $1" "$1" "${2-success}"; }
# An exploration whose explore job ended $2 hours ago with conclusion $1 (success, failure,
# cancelled, skipped), its step "Rounds ran" ending $3 (success unless given; skipped when the
# job ran no round; empty, no such step, when the job never ran).
explored() {
  run $((200 + $2)) proptest-explore.yml completed "filler proptest exploration" $(($2 + 1)) "$1"
  job $((200 + $2)) explore completed pve-silico-22-1 self-hosted,linux,x64,filler "$1" "$2" "${3-success}"
}
# Busy runners: real shards of a run of the merge lane on each of silico-$@.
shards_on() {
  run 7000 mutants.yml in_progress "mutants abc since main" 2
  job 7000 plan completed pve-silico-13-1 self-hosted,linux,x64,control success 2
  local n
  for n in "$@"; do job 7000 "shard ($n)" in_progress "pve-silico-$n-1" self-hosted,linux,x64,mutants-pr; done
}
# A filler mutants run going, a shard on each of silico-$@ (silico-23 alone, unless given).
filler_mutants() {
  run 8000 mutants.yml in_progress "filler b3 mutants abc in files x" 1
  job 8000 plan completed pve-silico-13-1 self-hosted,linux,x64,control success 1
  local n i=0
  for n in "${@-23}"; do
    job 8000 "shard ($i)" in_progress "pve-silico-$n-1" self-hosted,linux,x64,filler
    i=$((i + 1))
  done
}
# An exploration going (run 290), its explore job running on silico-$1, or, with none given,
# queued for a runner labelled filler.
exploring() {
  run 290 proptest-explore.yml in_progress "filler proptest exploration" 0
  job 290 room completed pve-silico-13-1 self-hosted,linux,x64,control success 0
  if [ -n "${1-}" ]; then
    job 290 explore in_progress "pve-silico-$1-1" self-hosted,linux,x64,filler
  else
    job 290 explore queued "" self-hosted,linux,x64,filler
  fi
}
# A filler sweep going (run 400), its sweep job running on silico-$1, or, with none given,
# queued for a runner labelled filler.
sweeping() {
  run 400 oracle-sweep.yml in_progress "filler oracle sweep, seed 400" 0
  if [ -n "${1-}" ]; then
    job 400 sweep in_progress "pve-silico-$1-1" self-hosted,linux,x64,filler
  else
    job 400 sweep queued "" self-hosted,linux,x64,filler
  fi
}
# A real mutants run's plan going (run 7100), on silico-14, a runner labelled control: the run
# that calls cancel-filler.sh with its lane and shards.
planning() {
  run 7100 mutants.yml in_progress "mutants def since main" 0
  job 7100 plan in_progress pve-silico-14-1 self-hosted,linux,x64,control
}
# GitHub failing every call whose arguments hold $1.
failing() { printf '%s' "$1" >"$STATE/fail"; }

fill() {
  (cd "$tmp/checkout" && PATH="$tmp/bin:$PATH" STATE=$STATE GH_TOKEN=x GH_REPO=o/r DRY=false BRANCH=main \
    GITHUB_RUN_ID=1 GITHUB_STEP_SUMMARY=$STATE/summary bash .github/fill.sh >"$STATE/log" 2>&1) ||
    { echo "fill.sh failed:" >&2; cat "$STATE/log" >&2; return 1; }
}
room() {
  (cd "$tmp/checkout" && PATH="$tmp/bin:$PATH" STATE=$STATE GH_TOKEN=x GH_REPO=o/r BRANCH=main \
    GITHUB_RUN_ID=1 GITHUB_STEP_SUMMARY=$STATE/summary GITHUB_OUTPUT=$STATE/output bash .github/proptest-room.sh >"$STATE/log" 2>&1) ||
    { echo "proptest-room.sh failed:" >&2; cat "$STATE/log" >&2; return 1; }
}
# cancel-filler.sh with arguments $@: a real mutants run's plan's (--lane LABEL --shards N),
# the filler's (workflow files), or none, the bench's and a night's sweep's.
cancel_filler() {
  (cd "$tmp/checkout" && PATH="$tmp/bin:$PATH" STATE=$STATE GH_TOKEN=x GH_REPO=o/r \
    bash .github/cancel-filler.sh "$@" >"$STATE/log" 2>&1) ||
    { echo "cancel-filler.sh failed:" >&2; cat "$STATE/log" >&2; return 1; }
}

failed=0
case_name=
check() {
  if ! "$@"; then
    echo "FAIL: $case_name: $*" >&2
    sed 's/^/  summary: /' "$STATE/summary" >&2
    sed 's/^/  calls: /' "$STATE/calls" >&2
    sed 's/^/  output: /' "$STATE/output" >&2
    if [ -f "$STATE/log" ]; then sed 's/^/  log: /' "$STATE/log" >&2; fi
    failed=1
  fi
}
said() { grep -qF -- "$1" "$STATE/summary"; }
called() { grep -qxF -- "$1" "$STATE/calls"; }
# What the decision changed on GitHub was these calls, and only these, in any order.
only_called() { [ "$(sort "$STATE/calls")" = "$(printf '%s\n' "$@" | sort)" ]; }
quiet() { [ ! -s "$STATE/calls" ]; }
logged() { grep -qF -- "$1" "$STATE/log"; }
unanswered() { ! grep -q '^go=' "$STATE/output"; }
# The decision $1 (fill or room) fails, as a failed look at GitHub must make it.
fails() { ! "$1" 2>/dev/null; }
answered() { [ "$(grep '^go=' "$STATE/output")" = "go=$1" ] && [ "$(grep '^overdue=' "$STATE/output")" = "overdue=$2" ]; }
cases=0
begin() { case_name=$1; cases=$((cases + 1)); state "$2"; }

# filler.yml: the oracle sweep.

begin "overdue sweep, no idle CPU, filler mutants going: they're cancelled, it starts" 0
swept 25
shards_on 5 6 7 8
filler_mutants
fill
check said "The oracle sweep is overdue"
check called "run cancel 8000"
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "overdue sweep, a gate waiting: nothing starts" 0
swept 25
run 6000 gate.yml queued "feat: x" 0
job 6000 gate queued "" self-hosted,linux,x64,pve,gate
fill
check said "Nothing started: the oracle sweep is overdue (none in a day but those cancelled), but a gate is waiting for a runner."
check quiet

begin "overdue sweep, a real mutants shard waiting that a free runner could take: nothing starts" 0
swept 25
shards_on 5 6
job 7000 "shard (2)" queued "" self-hosted,linux,x64,mutants-pr
fill
check said "Nothing started: a job asking for mutants-pr is queued, and a runner of that lane is free."
check quiet

begin "overdue sweep, a real mutants run starting: nothing starts" 0
swept 25
run 7100 mutants.yml in_progress "mutants def since main" 0
job 7100 plan in_progress pve-silico-14-1 self-hosted,linux,x64,control
fill
check said "Nothing started: a mutants run, a sweep, or a proptest exploration is starting."
check quiet

begin "overdue sweep, a real shard waiting behind its lane's busy runners: it starts on a free one" 0
swept 25
shards_on 5 6 7 8 9 10 11 12 15 16 17 20 21
job 7000 "shard (99)" queued "" self-hosted,linux,x64,mutants-pr
fill
check said "The oracle sweep is overdue"
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "overdue sweep, real work on every runner labelled filler: nothing starts" 0
swept 25
shards_on 5 6 7 8 9 10 11 12 22 23
fill
check said "Nothing started: the oracle sweep is overdue (none in a day but those cancelled), but real work holds every runner labelled filler."
check quiet

begin "overdue sweep, an exploration going: it waits for its end" 0
swept 25
run 210 proptest-explore.yml in_progress "filler proptest exploration" 0
job 210 room completed pve-silico-13-1 self-hosted,linux,x64,control success 0
job 210 explore in_progress pve-silico-22-1 self-hosted,linux,x64,filler
fill
check said "Nothing started: the oracle sweep is overdue (none in a day but those cancelled), waiting for a proptest exploration to end."
check quiet

begin "sweep merely due, no idle CPU: nothing starts" 1
swept 5
filler_mutants
fill
check said "Nothing started: 1 of 30 CPUs are idle, a fifth or less."
check quiet

begin "sweep merely due, idle CPU, nothing going: it starts, as before" 20
swept 5
run 8100 mutants.yml completed "filler a mutants 1111111111111111111111111111111111111111 since 0000" 3
fill
check said "Room for 2 shards"
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "no sweep in four hours or a day, ever: overdue" 0
fill
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "both overdue: the sweep starts" 0
swept 25
explored success 30
fill
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "overdue sweep, real work on silico-5 to -12, filler mutants on -22 and -23: they're cancelled, it starts" 0
swept 25
shards_on 5 6 7 8 9 10 11 12
filler_mutants 22 23
fill
check said "The oracle sweep is overdue"
check called "run cancel 8000"
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "a sweep cancelled today doesn't count: overdue, it starts" 0
swept 25
swept 2 cancelled
fill
check said "The oracle sweep is overdue: no filler sweep in a day but those cancelled"
check called "workflow run oracle-sweep.yml --ref main -f filler=true"

begin "a failed look at the queued runs: fill fails, starting nothing" 0
failing "runs?status=queued"
check fails fill
check logged "stub gh: failing api repos/o/r/actions/runs?status=queued"
check quiet

begin "a failed look at one run's jobs, not the last: fill fails, starting or cancelling nothing" 0
swept 25
shards_on 5 6
job 7000 "shard (2)" queued "" self-hosted,linux,x64,mutants-pr
filler_mutants
failing "actions/runs/7000/jobs"
check fails fill
check logged "stub gh: failing api repos/o/r/actions/runs/7000/jobs"
check quiet

# proptest-room.sh: the exploration.

begin "overdue exploration, no idle CPU: it goes" 0
swept 1
explored success 30
shards_on 5 6 7 8
room
check said "Overdue: no exploration has passed or failed in a day."
check said "Room, overdue: 4 of 10 runners labelled filler busy, whatever CPU is idle."
check answered true true

begin "overdue exploration, a gate waiting: no" 0
swept 1
explored success 30
run 6000 gate.yml queued "feat: x" 0
room
check said "Not started: a gate is waiting for a runner."
check answered false true

begin "overdue exploration, a real mutants shard waiting that a free runner could take: no" 0
swept 1
explored success 30
shards_on 5 6
job 7000 "shard (2)" queued "" self-hosted,linux,x64,mutants-pr
room
check said "Not started: a real mutants run (7000) has a shard waiting for mutants-pr, and a runner of that lane is free."
check answered false true

begin "overdue exploration, a real shard waiting behind its lane's busy runners: it goes" 0
swept 1
explored success 30
shards_on 5 6 7 8 9 10 11 12 15 16 17 20 21
job 7000 "shard (99)" queued "" self-hosted,linux,x64,mutants-pr
room
check answered true true

begin "overdue exploration, a real mutants run starting: no" 0
swept 1
explored success 30
run 7100 mutants.yml in_progress "mutants def since main" 0
job 7100 plan in_progress pve-silico-14-1 self-hosted,linux,x64,control
room
check said "Not started: a real mutants run (7100) is starting or has a shard waiting for a runner."
check answered false true

begin "overdue exploration, every runner labelled filler busy: no" 0
swept 1
explored success 30
shards_on 5 6 7 8 9 10 11 12 22 23
room
check said "Not started: all 10 runners labelled filler are busy."
check answered false true

begin "overdue exploration, sweep merely due: the exploration goes" 0
swept 5
explored success 30
room
check answered true true

begin "both overdue: the sweep goes first" 30
swept 25
explored success 30
room
check said "Not started: filler's oracle sweep is overdue too, and goes first."
check answered false true

begin "an exploration cancelled or skipped today doesn't count: still overdue" 0
swept 1
explored success 30
explored cancelled 2
explored skipped 6 ""
room
check answered true true

begin "an exploration that passed today with no round doesn't count: still overdue" 0
swept 1
explored success 30
explored success 3 skipped
room
check said "Overdue: no exploration has passed or failed in a day."
check answered true true

begin "a sweep cancelled today doesn't count, the exploration overdue too: the sweep goes first" 30
swept 2 cancelled
explored success 30
room
check said "Not started: filler's oracle sweep is overdue too, and goes first."
check answered false true

begin "a sweep cancelled today doesn't count, the exploration not overdue: the sweep goes first" 30
swept 2 cancelled
explored success 3
room
check said "Not started: filler's oracle sweep is overdue, and goes first."
check answered false false

begin "a sweep cancelled within four hours still takes that turn: not due, the exploration goes" 20
swept 5
swept 2 cancelled
explored success 3
room
check said "Room: 0 of 10 runners labelled filler busy, 20 of 30 CPUs idle."
check answered true false

begin "a failed look at the queued runs: room fails, answering nothing" 0
swept 1
explored success 30
failing "runs?status=queued"
check fails room
check logged "stub gh: failing api repos/o/r/actions/runs?status=queued"
check unanswered

begin "a failed look at one run's jobs, not the last: room fails, answering nothing" 0
swept 1
explored success 30
shards_on 5 6
job 7000 "shard (2)" queued "" self-hosted,linux,x64,mutants-pr
filler_mutants
failing "actions/runs/7000/jobs"
check fails room
check logged "stub gh: failing api repos/o/r/actions/runs/7000/jobs"
check unanswered

begin "an exploration that failed today counts: not overdue, and no idle CPU" 1
swept 1
explored failure 3
room
check said "Not started: 1 of 30 CPUs are idle, a fifth or less."
check answered false false

begin "not overdue, a real shard waiting behind its lane's busy runners: no, as before" 30
swept 1
explored success 3
shards_on 5 6 7 8 9 10 11 12 15 16 17 20 21
job 7000 "shard (99)" queued "" self-hosted,linux,x64,mutants-pr
room
check said "Not started: a real mutants run (7000) is starting or has a shard waiting for a runner."
check answered false false

begin "not overdue, sweep due: no, as before" 30
swept 5
explored success 3
room
check said "Not started: filler's oracle sweep is due, and goes first."
check answered false false

begin "not overdue, idle CPU, room: it goes, as before" 20
swept 1
explored success 3
room
check said "Room: 0 of 10 runners labelled filler busy, 20 of 30 CPUs idle."
check answered true false

# cancel-filler.sh: what real work cancels. The lanes (.github/lanes.sh): mutants-pr, silico-5
# to -12, -15 to -17, -20, -21; mutants, -9 to -12 and -15 to -17; filler, -5 to -12, -22, -23.
# An exploration is overdue here when none ended, having run a round, in a day (explored 30:
# the last one 30 hours ago); a sweep when none ended in a day but those cancelled (swept 25).

begin "a real run of 2 shards, the overdue exploration on silico-8, 4 of its lane free once filler mutants are cancelled: they're cancelled, it's kept" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
cancel_filler --lane mutants-pr --shards 2
# Busy: 8 (the exploration), 10 to 12, 15 to 17, 20, 21 (real); free: 5, 6, 7 (filler mutants,
# cancelled), and 9.
check logged "4 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 4 spare for 2 shards"
check logged "Keeping the exploration 290, overdue"
check only_called "run cancel 8000"

begin "as many runners free as shards: the overdue exploration is kept" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
cancel_filler --lane mutants-pr --shards 4
check only_called "run cancel 8000"

begin "one runner fewer free than shards, the overdue exploration on one of their lane: it's cancelled" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
cancel_filler --lane mutants-pr --shards 5
check logged "4 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 4 spare for 5 shards"
check only_called "run cancel 8000" "run cancel 290"

begin "the lane all busy with real work but the overdue exploration's runner: it's cancelled" 0
explored success 30
planning
shards_on 5 6 7 9 10 11 12 15 16 17 20 21
filler_mutants 22 23
exploring 8
cancel_filler --lane mutants-pr --shards 2
check logged "0 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 0 spare for 2 shards"
check only_called "run cancel 8000" "run cancel 290"

begin "the lane all busy, the overdue exploration on silico-22, which the lane hasn't: it's kept" 0
explored success 30
planning
shards_on 5 6 7 8 9 10 11 12 15 16 17 20 21
exploring 22
cancel_filler --lane mutants-pr --shards 2
check logged "Keeping the exploration 290, overdue: it's on a runner the shards' lane hasn't"
check quiet

begin "the lane all busy, the overdue exploration queued for a runner: it's cancelled" 0
explored success 30
planning
shards_on 5 6 7 8 9 10 11 12 15 16 17 20 21
exploring
cancel_filler --lane mutants-pr --shards 1
check only_called "run cancel 290"

begin "a merge-lane run, its lane busy, the overdue exploration on silico-5: it's cancelled" 0
explored success 30
planning
shards_on 6 7 8 9 10 11 12 15 16 17 20 21
exploring 5
cancel_filler --lane mutants-pr --shards 1
check only_called "run cancel 290"

begin "a long run, its lane busy, the overdue exploration on silico-5, which the long lane hasn't: it's kept" 0
explored success 30
planning
shards_on 6 7 8 9 10 11 12 15 16 17 20 21
exploring 5
cancel_filler --lane mutants --shards 1
check logged "0 runners labelled mutants free once filler mutants are cancelled, 0 jobs waiting for one: 0 spare for 1 shards"
check quiet

begin "an exploration not overdue is cancelled, every runner free" 0
explored success 3
planning
exploring 22
cancel_filler --lane mutants-pr --shards 1
check logged "Cancelling filler run 290: the exploration isn't overdue"
check only_called "run cancel 290"

begin "an overdue filler sweep (one cancelled today doesn't count), runners free: it's kept" 0
swept 25
swept 2 cancelled
planning
filler_mutants 22 23
sweeping 9
cancel_filler --lane mutants-pr --shards 2
# Free: 13 but 9.
check logged "12 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 12 spare for 2 shards"
check logged "Keeping the filler sweep 400, overdue"
check only_called "run cancel 8000"

begin "an overdue filler sweep, the lane all busy but its runner: it's cancelled" 0
swept 25
planning
shards_on 5 6 7 8 10 11 12 15 16 17 20 21
sweeping 9
cancel_filler --lane mutants-pr --shards 1
check only_called "run cancel 400"

begin "a filler sweep not overdue is cancelled, every runner free" 0
swept 5
planning
sweeping 22
cancel_filler --lane mutants-pr --shards 1
check logged "Cancelling filler run 400: the filler sweep isn't overdue"
check only_called "run cancel 400"

begin "a failed look at the runs going: the overdue exploration is cancelled, warned of, and the real run goes on" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
failing "runs?status=queued"
check cancel_filler --lane mutants-pr --shards 2
check logged "::warning::couldn't tell whether the shards need the overdue exploration 290's runner: cancelled, as real work comes first"
check only_called "run cancel 8000" "run cancel 290"

begin "a failed look at a run's jobs: the overdue exploration is cancelled, warned of, and the real run goes on" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
failing "actions/runs/7000/jobs"
check cancel_filler --lane mutants-pr --shards 2
check logged "::warning::couldn't tell whether the shards need the overdue exploration 290's runner"
check only_called "run cancel 8000" "run cancel 290"

begin "a failed look at the past explorations: the exploration is cancelled, warned of, and the real run goes on" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
failing "actions/runs/230/jobs"
check cancel_filler --lane mutants-pr --shards 2
check logged "::warning::couldn't tell whether the exploration 290 is overdue: cancelled, as real work comes first"
check only_called "run cancel 8000" "run cancel 290"

begin "an exploration started by hand, waiting for a runner, as many spare as shards: it's cancelled, as it would take one" 0
explored success 30
planning
shards_on 8 9 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
# Started by hand, it skips the room (proptest-explore.yml) and queues at once.
run 290 proptest-explore.yml in_progress "filler proptest exploration" 0
job 290 room completed "" self-hosted,linux,x64,control skipped 0
job 290 explore queued "" self-hosted,linux,x64,filler
cancel_filler --lane mutants-pr --shards 3
# Free: 5, 6, 7 (filler mutants, cancelled); the exploration's job asks for filler, not
# mutants-pr, so isn't counted waiting, but would take one of them.
check logged "3 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 3 spare for 3 shards"
check only_called "run cancel 8000" "run cancel 290"

begin "an exploration started by hand, waiting for a runner, one more spare than shards: it's kept" 0
explored success 30
planning
shards_on 8 9 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
run 290 proptest-explore.yml in_progress "filler proptest exploration" 0
job 290 room completed "" self-hosted,linux,x64,control skipped 0
job 290 explore queued "" self-hosted,linux,x64,filler
cancel_filler --lane mutants-pr --shards 2
check logged "Keeping the exploration 290, overdue: the lane has a runner spare for it and each shard"
check only_called "run cancel 8000"

begin "an overdue filler sweep waiting for a runner, as many spare as shards: it's cancelled" 0
swept 25
planning
shards_on 8 9 10 11 12 15 16 17 20 21
sweeping
cancel_filler --lane mutants-pr --shards 3
# Free: 5, 6, 7.
check only_called "run cancel 400"

begin "an overdue sweep and exploration both waiting, one more spare than shards: the first kept takes it, the other is cancelled" 0
explored success 30
swept 25
planning
shards_on 8 9 10 11 12 15 16 17 20 21
sweeping
run 290 proptest-explore.yml in_progress "filler proptest exploration" 0
job 290 explore queued "" self-hosted,linux,x64,filler
cancel_filler --lane mutants-pr --shards 2
# Spare: 5, 6, 7. The sweep, listed first, is kept and will take one; then 2 are spare for 2
# shards, none for the exploration.
check logged "Keeping the filler sweep 400, overdue: the lane has a runner spare for it and each shard"
check only_called "run cancel 290"

begin "another real run's shard waiting for the lane: one fewer spare, and the overdue exploration on silico-8 is cancelled" 0
explored success 30
planning
shards_on 9 10 11 12 15 16 17 20 21
filler_mutants 5 6
exploring 8
run 7200 mutants.yml in_progress "mutants ghi since main" 1
job 7200 plan completed pve-silico-13-1 self-hosted,linux,x64,control success 1
job 7200 "shard (0)" queued "" self-hosted,linux,x64,mutants-pr
cancel_filler --lane mutants-pr --shards 3
# Free: 5, 6 (filler mutants, cancelled), and 7; waiting: run 7200's shard.
check logged "3 runners labelled mutants-pr free once filler mutants are cancelled, 1 jobs waiting for one: 2 spare for 3 shards"
check only_called "run cancel 8000" "run cancel 290"

begin "another real run's shard waiting for the lane, and a gate, which asks for other runners: the overdue exploration is kept for 2 shards" 0
explored success 30
planning
shards_on 9 10 11 12 15 16 17 20 21
filler_mutants 5 6
exploring 8
run 7200 mutants.yml in_progress "mutants ghi since main" 1
job 7200 plan completed pve-silico-13-1 self-hosted,linux,x64,control success 1
job 7200 "shard (0)" queued "" self-hosted,linux,x64,mutants-pr
run 6000 gate.yml queued "feat: x" 0
job 6000 gate queued "" self-hosted,linux,x64,pve,gate
cancel_filler --lane mutants-pr --shards 2
check logged "3 runners labelled mutants-pr free once filler mutants are cancelled, 1 jobs waiting for one: 2 spare for 2 shards"
check only_called "run cancel 8000"

begin "a job that ended on a runner of the lane leaves it free: the overdue exploration is kept, as many spare as shards" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
job 7000 "shard (9)" completed pve-silico-9-1 self-hosted,linux,x64,mutants-pr success 1
filler_mutants 5 6 7
exploring 8
cancel_filler --lane mutants-pr --shards 4
# Free: 5, 6, 7 (filler mutants, cancelled), and 9.
check logged "4 runners labelled mutants-pr free once filler mutants are cancelled, 0 jobs waiting for one: 4 spare for 4 shards"
check only_called "run cancel 8000"

begin "a filler mutants run GitHub wouldn't cancel: the overdue exploration is cancelled, warned of, and the real run goes on" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
failing "run cancel 8000"
check cancel_filler --lane mutants-pr --shards 2
check logged "::warning::couldn't cancel filler run 8000"
check logged "::warning::couldn't tell whether the shards need the overdue exploration 290's runner: cancelled, as real work comes first"
check only_called "run cancel 290"

begin "a failed list of the past explorations: the exploration is cancelled, warned of, and the real run goes on" 0
explored success 30
planning
shards_on 10 11 12 15 16 17 20 21
filler_mutants 5 6 7
exploring 8
failing "proptest-explore.yml --branch"
check cancel_filler --lane mutants-pr --shards 2
check logged "::warning::couldn't tell whether the exploration 290 is overdue: cancelled, as real work comes first"
check only_called "run cancel 8000" "run cancel 290"

begin "the bench's call, with no arguments: every filler run cancelled, overdue or not, and nothing else" 0
explored success 30
swept 25
shards_on 10
run 500 oracle-sweep.yml in_progress "oracle sweeps and corpus, seed 500" 0
filler_mutants 22
exploring 8
sweeping 9
cancel_filler
check only_called "run cancel 8000" "run cancel 290" "run cancel 400"

begin "the filler's call, for filler mutants alone: only they are cancelled" 0
explored success 30
swept 25
filler_mutants 22
exploring 8
sweeping 9
cancel_filler mutants.yml
check only_called "run cancel 8000"

if [ "$failed" -ne 0 ]; then
  echo "filler-decisions: some cases failed" >&2
  exit 1
fi
echo "filler-decisions: every case passed ($cases)"
