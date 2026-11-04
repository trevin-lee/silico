#!/usr/bin/env bash
# The workspace's tests on the committed tree with new proptest seeds, in rounds: where the
# gate meets the same cases every run (pixi.toml's test task fixes PROPTEST_RNG_SEED), this
# looks for the cases it never meets (.github/workflows/proptest-explore.yml runs it on idle
# runners). Each round draws a random u64 for PROPTEST_RNG_SEED, says it, and runs every test
# but those tests/mutants.sh leaves out for holding to a wall clock or GiBs (its $filter): no
# filter tells a property's tests from the rest, which share their binaries and ignore the
# seed. Each property takes its own case count each round (not PROPTEST_CASES, which proptest
# lets override every test's own, so a property that takes 32 costly cases would take the
# variable's count, and one that takes 20,000 fewer): four rounds meet four times the cases. A
# round starts only while one more, as long as the longest yet, would end within the minutes
# given, the build not counted; the first failing round is the last. In the gate's worktree
# (tests/committed-tree.sh), as mutants are, so it never edits a checkout someone works in.
#
# Writes to OUTPUT (absolute: the tree is elsewhere): rounds.tsv (each round's seed, nextest's
# exit status, and seconds), round-N.xml (its JUnit report), and, for a round whose tests
# failed, failures/ (each failed test's issue title and body, tests/proptest-failures.py) and
# failures.md. Exits 0 when every round passed, or nextest's status for the round that didn't.
#
# Run through pixi:  pixi run proptest-explore -- /tmp/explore          (new seeds)
#                    pixi run proptest-explore -- /tmp/explore SEED     (one round at SEED)
# PROPTEST_EXPLORE_ROUNDS (4) and PROPTEST_EXPLORE_MINUTES (60) bound it. To run one failing
# test again, `PROPTEST_RNG_SEED=SEED pixi run cargo nextest run -p ...`, which each failure's
# body gives in full; not `pixi run test`, whose task fixes the seed.
set -euo pipefail

# Before the tree's checkout, which cds away from this script's directory.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# The tests mutants leave out ($filter): this sources tests/mutants.sh for it alone.
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

pixi run occt
pixi install -e oracle
mkdir -p "$out"
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
} >"$nextest_config"
nextest=(cargo nextest run --workspace --config-file "$nextest_config" --profile silico-explore)
junit=$out/junit.xml

echo "proptest-explore: testing $commit"
"${nextest[@]}" --no-run
printf 'round\tseed\tstatus\tseconds\n' >"$out/rounds.tsv"
start=$SECONDS
longest=0
round=1
while :; do
  this=${seed:-$(od -An -N8 -tu8 /dev/urandom | tr -d ' \n')}
  report "Round $round of at most $rounds: PROPTEST_RNG_SEED=$this"
  rm -f "$junit"
  began=$SECONDS
  status=0
  PROPTEST_RNG_SEED=$this "${nextest[@]}" -E "$filter" || status=$?
  took=$((SECONDS - began))
  printf '%s\t%s\t%s\t%s\n' "$round" "$this" "$status" "$took" >>"$out/rounds.tsv"
  if [ -f "$junit" ]; then mv "$junit" "$out/round-$round.xml"; fi
  if [ "$status" -ne 0 ]; then
    if [ -f "$out/round-$round.xml" ]; then
      python3 -I "$here/proptest-failures.py" "$out/round-$round.xml" "$this" "$commit" "$out"
    fi
    if [ -s "$out/failures.md" ]; then
      report "Round $round failed, at seed $this:"
      if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then cat "$out/failures.md" >>"$GITHUB_STEP_SUMMARY"; fi
      cat "$out/failures.md"
    else
      report "Round $round failed, at seed $this, with no failed test reported: nextest exited $status (see the log)"
    fi
    exit "$status"
  fi
  report "Round $round passed, in $took s"
  if [ "$took" -gt "$longest" ]; then longest=$took; fi
  if [ "$round" -ge "$rounds" ]; then break; fi
  if [ $((SECONDS - start + longest)) -gt $((minutes * 60)) ]; then
    report "Stopped after $round rounds: another as long as the longest wouldn't end within $minutes minutes"
    break
  fi
  round=$((round + 1))
done
report "Every round passed, at $commit"
