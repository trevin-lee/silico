#!/usr/bin/env bash
# The workspace's tests on the committed tree with new proptest seeds, in rounds: where the
# gate meets the same cases every run (pixi.toml's test task fixes PROPTEST_RNG_SEED), this
# looks for the cases it never meets (.github/workflows/proptest-explore.yml runs it on idle
# runners). Each round draws a random u64 for PROPTEST_RNG_SEED, says it, and runs every test
# but those tests/mutants.sh leaves out for holding to a wall clock or GiBs (its
# $explore_filter): no filter tells a property's tests from the rest, which share their
# binaries and ignore the seed. The slow proptests mutants leave out ($slow) run here, each
# given 20 minutes a try and no retry. Each property takes its own case count each round (not PROPTEST_CASES, which proptest
# lets override every test's own, so a property that takes 32 costly cases would take the
# variable's count, and one that takes 20,000 fewer): four rounds meet four times the cases.
# About an hour in all, as a filler mutants shard has, the build included: a round starts
# only in the first three quarters of the minutes, and only if one as long as the longest yet
# would end within them, and the last is cut off at their end. The first failing round is the
# last: each property failure in it (if no more than five) is run again alone at its seed,
# within a quarter hour more, and only one that fails again, giving an input, gets an issue of
# its own; the rest go to one issue (tests/proptest-failures.py). In the gate's worktree
# (tests/committed-tree.sh), as mutants are, so it never edits a checkout someone works in.
#
# Writes to OUTPUT (absolute: the tree is elsewhere): rounds.tsv (each round's seed, nextest's
# exit status, and seconds), round-N.xml (its JUnit report), and, for a round whose tests
# failed, reruns.tsv and rerun-N.xml (what each run alone found, and its report), failures/
# (each issue's title and body) and failures.md; and outcome, how far it got (below). Exits 0
# when no test failed (a round cut off at the end too), 1 when tests failed, or nextest's
# status when it failed naming no test.
#
# Run through pixi:  pixi run proptest-explore -- /tmp/explore          (new seeds)
#                    pixi run proptest-explore -- /tmp/explore SEED     (one round at SEED)
# PROPTEST_EXPLORE_ROUNDS (4) and PROPTEST_EXPLORE_MINUTES (60) bound it; the minutes are kept
# where `timeout` is (Linux), and only between rounds elsewhere. PROPTEST_EXPLORE_SLOW_ALONE,
# set and not empty, runs each slow proptest alone (below). To run one failing
# test again, `PROPTEST_RNG_SEED=SEED pixi run cargo nextest run -p ...`, which each failure's
# body gives in full; not `pixi run test`, whose task fixes the seed.
set -euo pipefail

# Before the tree's checkout, which cds away from this script's directory.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# The tests left out ($explore_filter) and the slow proptests ($slow): this sources
# tests/mutants.sh for them alone.
# shellcheck source=tests/mutants.sh
source "$here/mutants.sh"

usage() {
  echo "usage: tests/proptest-explore.sh OUTPUT [SEED]  (OUTPUT an absolute directory; SEED a u64)" >&2
  exit 2
}
# A u64 in decimal, as proptest reads PROPTEST_RNG_SEED. Anything else it warns about and
# ignores, each test keeping its own seed (random, for most), which no one could then repeat.
is_u64() {
  case $1 in
  '' | *[!0-9]*) return 1 ;;
  esac
  # Without leading zeros, so the comparison below is by length, then text.
  [ "$1" = 0 ] || [ "${1#0}" = "$1" ] || return 1
  [ "${#1}" -lt 20 ] || { [ "${#1}" -eq 20 ] && [[ ! $1 > 18446744073709551615 ]]; }
}
positive() {
  case $1 in
  '' | *[!0-9]* | 0*) return 1 ;;
  esac
}
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
out=$1
seed=${2-}
# Absolute, and nothing TOML would have to escape: the report's path is written into
# nextest's config below.
case $out in
*'"'* | *'\'* | *$'\n'*) usage ;;
/*) ;;
*) usage ;;
esac
if [ -n "$seed" ] && ! is_u64 "$seed"; then
  echo "proptest-explore: the seed must be a u64 in decimal, without leading zeros: '$seed'" >&2
  exit 2
fi
rounds=${PROPTEST_EXPLORE_ROUNDS:-4}
minutes=${PROPTEST_EXPLORE_MINUTES:-60}
if ! positive "$rounds" || ! positive "$minutes"; then
  echo "proptest-explore: PROPTEST_EXPLORE_ROUNDS and PROPTEST_EXPLORE_MINUTES must be positive integers" >&2
  exit 2
fi
if [ -n "$seed" ]; then rounds=1; fi

# shellcheck source=tests/committed-tree.sh
source "$here/committed-tree.sh"

# To the log, and to the run's summary on CI.
report() {
  echo "proptest-explore: $1"
  if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then echo "$1" >>"$GITHUB_STEP_SUMMARY"; fi
}

# The exploration's clock: the build counts, as a filler mutants shard's hour does.
start=$SECONDS
pixi run occt
pixi install -e oracle
mkdir -p "$out"
# Nothing an earlier run left here is read as this one's.
rm -rf "$out/failures" "$out/failures.md" "$out/reruns.tsv" "$out/junit.xml" "$out"/round-*.xml "$out"/rerun-*.xml
# How far it got, for the report (.github/proptest-report.sh): setup, until the rounds start;
# rounds, while they run; then passed or failed. A run stopped mid-round from outside (its
# step's time limit) leaves rounds, which is no setup failure.
echo setup >"$out/outcome"
# The tree's own config with a profile added, as tests/mutants.sh adds its own: each test 5
# minutes a try, at the idle priority CI gives it, with two retries, so a test only load
# slowed passes on one (a property's failure, its seed fixed, fails all three); every test
# run; and a JUnit report, which keeps each failed test's output, written to OUTPUT (nextest
# takes an absolute path as it is, wherever its store is).
nextest_config=$(mktemp "${TMPDIR:-/tmp}/silico-explore-nextest.XXXXXX")
trap 'rm -f "$nextest_config"' EXIT
{
  cat .config/nextest.toml
  printf '\n[profile.silico-explore]\nfail-fast = false\nretries = 2\n'
  printf 'slow-timeout = { period = "60s", terminate-after = 5 }\n'
  printf '\n[profile.silico-explore.junit]\npath = "%s"\n' "$out/junit.xml"
  # The slow proptests (tests/mutants.sh's $slow): beside the others, as the gate runs them,
  # given 20 minutes a try, and no retry, since 20 minutes covers load and a property's
  # failure is run again alone below.
  if [ -n "$slow" ]; then
    printf '\n[[profile.silico-explore.overrides]]\nfilter = "%s"\n' "$slow"
    printf 'retries = 0\n'
    printf 'slow-timeout = { period = "120s", terminate-after = 10 }\n'
    # Alone, nothing else of this run beside it, when the exploration was overdue
    # (.github/proptest-room.sh) and may have started on a busy VM: its runner at the idle
    # weight then gets about an equal share with each busy runner's (2 or 3 CPUs of 30), and a
    # slow proptest (some 357 s alone, 3 threads with lavapipe's) beside six other tests
    # gets a third of that, past its 20 minutes. Alone, it has the share to itself.
    if [ -n "${PROPTEST_EXPLORE_SLOW_ALONE-}" ]; then
      printf 'threads-required = "num-test-threads"\n'
    fi
  fi
} >"$nextest_config"
nextest=(cargo nextest run --config-file "$nextest_config" --profile silico-explore)
junit=$out/junit.xml
# Rounds start only in the first three quarters of the minutes, and end, cut off if they
# must, at the minutes' end; a failure's runs alone (below) have a quarter hour more.
last_start=$((start + minutes * 45))
end=$((start + minutes * 60))
reruns_end=$((end + 900))
# The most property failures run again alone, each for at most 10 minutes: past that many,
# the run reports them all in one issue, unrun (proptest-failures.py's MOST_ISSUES).
most_issues=5
rerun_most=600
rerun_slow_most=1200

# Runs "$@" for at most $1 seconds, where `timeout` is (CI's Linux; not macOS's base system,
# where it runs unbounded). Its status 124 says the time ran out.
within() {
  local seconds=$1
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout --kill-after=120 "$seconds" "$@"
  else
    "$@"
  fi
}

# The cases proptest saved in the tree this run (beside an integration test, or under a
# crate's proptest-regressions/), put back as committed: a test run again alone then meets
# what a run of its command at the commit meets, the committed cases and the seed's.
reset_saved_cases() {
  local f paths=('*.proptest-regressions' '*/proptest-regressions/*')
  git diff --name-only -- "${paths[@]}" | while IFS= read -r f; do git checkout --quiet -- "$f"; done
  git ls-files --others -- "${paths[@]}" | while IFS= read -r f; do rm -f -- "$f"; done
}

# A round's failed tests, sorted, each property failure run again alone at the round's seed
# (at most most_issues of them), and written as issues (tests/proptest-failures.py). Returns
# 1 if the round's report can't be read.
failures() {
  local xml=$1 this=$2 list properties n=0 kind binary test args left status result
  list=$(python3 -I "$here/proptest-failures.py" list "$xml") || return 1
  : >"$out/reruns.tsv"
  properties=$(awk -F '\t' '$1 == "property" { n++ } END { print n + 0 }' <<<"$list")
  if [ "$properties" -gt 0 ] && [ "$properties" -le "$most_issues" ]; then
    reset_saved_cases
    while IFS=$'\t' read -r -u 3 kind binary test args; do
      [ "$kind" = property ] || continue
      n=$((n + 1))
      left=$((reruns_end - SECONDS))
      if [ "$left" -le 0 ]; then
        result=unrun
      else
        rm -f "$junit"
        status=0
        # $args is "-p <crate> <cargo's flag for the binary>", split as words.
        # shellcheck disable=SC2086
        most=$rerun_most
        # A slow proptest needs up to its 20 minutes again.
        for slow_test in "${slow_proptests[@]+"${slow_proptests[@]}"}"; do
          if [ "$test" = "$slow_test" ]; then most=$rerun_slow_most; fi
        done
        PROPTEST_RNG_SEED=$this within $((left < most ? left : most)) \
          "${nextest[@]}" --retries 0 $args -E "test(=$test)" </dev/null || status=$?
        if [ "$status" -eq 0 ]; then
          result=passed
        elif [ -f "$junit" ] && python3 -I "$here/proptest-failures.py" failed-again "$junit" "$binary" "$test"; then
          result=again
        else
          result=no-input
        fi
        if [ -f "$junit" ]; then mv "$junit" "$out/rerun-$n.xml"; fi
      fi
      report "Run again alone: $binary $test: $result"
      printf '%s\t%s\t%s\n' "$binary" "$test" "$result" >>"$out/reruns.tsv"
    done 3<<<"$list"
  fi
  python3 -I "$here/proptest-failures.py" render "$xml" "$this" "$commit" "$out" "$out/reruns.tsv"
}

echo "proptest-explore: testing $commit"
"${nextest[@]}" --workspace --no-run
printf 'round\tseed\tstatus\tseconds\n' >"$out/rounds.tsv"
# No time left for a round passes, but explores nothing: the rounds.tsv it leaves, its header
# alone, keeps an overdue exploration overdue (proptest-explore.yml's "Rounds ran",
# .github/proptest-room.sh).
if [ "$SECONDS" -ge "$last_start" ]; then
  report "No round: the build ended past $((minutes * 3 / 4)) minutes"
  echo passed >"$out/outcome"
  exit 0
fi
echo rounds >"$out/outcome"
longest=0
round=1
while :; do
  this=${seed:-$(od -An -N8 -tu8 /dev/urandom | tr -d ' \n')}
  report "Round $round of at most $rounds: PROPTEST_RNG_SEED=$this"
  rm -f "$junit"
  began=$SECONDS
  status=0
  PROPTEST_RNG_SEED=$this within $((end - SECONDS > 1 ? end - SECONDS : 1)) \
    "${nextest[@]}" --workspace -E "$explore_filter" || status=$?
  took=$((SECONDS - began))
  printf '%s\t%s\t%s\t%s\n' "$round" "$this" "$status" "$took" >>"$out/rounds.tsv"
  if [ -f "$junit" ]; then mv "$junit" "$out/round-$round.xml"; fi
  if [ "$status" -ne 0 ]; then
    if [ "$status" -eq 124 ]; then report "Round $round, at seed $this, was cut off at $minutes minutes"; fi
    if [ -f "$out/round-$round.xml" ]; then
      failures "$out/round-$round.xml" "$this" || report "Couldn't read round-$round.xml: see the log"
    fi
    if [ -s "$out/failures.md" ]; then
      report "Round $round failed, at seed $this:"
      if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then cat "$out/failures.md" >>"$GITHUB_STEP_SUMMARY"; fi
      cat "$out/failures.md"
      echo failed >"$out/outcome"
      exit 1
    fi
    # Cut off with nothing failed: the time ran out, which is no failure.
    if [ "$status" -eq 124 ]; then break; fi
    report "Round $round failed, at seed $this, with no failed test reported: nextest exited $status (see the log)"
    echo setup >"$out/outcome"
    exit "$status"
  fi
  report "Round $round passed, in $took s"
  if [ "$took" -gt "$longest" ]; then longest=$took; fi
  if [ "$round" -ge "$rounds" ]; then break; fi
  if [ "$SECONDS" -ge "$last_start" ] || [ $((SECONDS + longest)) -gt "$end" ]; then
    report "Stopped after $round rounds: another wouldn't start within $((minutes * 3 / 4)) minutes, or end within $minutes"
    break
  fi
  round=$((round + 1))
done
report "Done, at $commit: no test failed"
echo passed >"$out/outcome"
