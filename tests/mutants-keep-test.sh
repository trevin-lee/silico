#!/usr/bin/env bash
# .github/mutants-keep.sh, which keeps a mutants shard's outcomes on the VM for the run's
# summary, against a made-up mutants.out shaped as cargo-mutants 27.1.0 writes it (its
# outcomes.json read from a real run of it on 2026-11-10: "Baseline" or {"Mutant": ...} as
# the scenario, a summary of Success, CaughtMutant, MissedMutant, Timeout, Unviable or
# Failure, and each log_path under log/). Each case says what must be kept and what mustn't.
# Needs bash and python3; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/mutants-keep-test.sh)
set -euo pipefail

# What a job's environment says about the run is the cases' to give, never the job's: the
# gate runs this test, and a re-run of the gate (GITHUB_RUN_ATTEMPT=2) once failed it, the
# marker kept saying the gate's attempt where a case wants the default's. Nor does a case
# write the gate's own summary (GITHUB_STEP_SUMMARY).
unset GITHUB_RUN_ATTEMPT GITHUB_STEP_SUMMARY
# Whether this is the run the test makes of itself below: an argument, so no environment
# skips that case.
inner=${1:-}

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/silico-mutants-keep.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
keep=$repo/.github/mutants-keep.sh

fail() {
  echo "mutants-keep: $*" >&2
  exit 1
}

passed=0
pass() {
  passed=$((passed + 1))
}

# A shard's mutants.out: a mutant of each outcome, its lists, and a log for each.
out=$tmp/out/mutants.out
mkdir -p "$out/log" "$out/diff"
printf 'a.rs:1:1: caught one\n' >"$out/caught.txt"
printf 'a.rs:2:1: missed one\n' >"$out/missed.txt"
printf 'a.rs:3:1: timed out\n' >"$out/timeout.txt"
printf 'a.rs:4:1: unviable one\n' >"$out/unviable.txt"
echo '[{"name": "a.rs:1:1"}, {"name": "a.rs:2:1"}, {"name": "a.rs:3:1"}, {"name": "a.rs:4:1"}, {"name": "a.rs:5:1"}]' >"$out/mutants.json"
echo 0 >"$out/shard"
echo "4 threads" >"$out/threads"
for log in baseline caught missed timeout unviable failure; do echo "the $log log" >"$out/log/$log.log"; done
echo "a diff" >"$out/diff/missed.diff"
echo secret >"$tmp/out/outside.log"
cat >"$out/outcomes.json" <<'JSON'
{"cargo_mutants_version": "27.1.0", "total_mutants": 5, "caught": 1, "missed": 1, "timeout": 1, "unviable": 1, "success": 1,
 "outcomes": [
  {"scenario": "Baseline", "summary": "Success", "log_path": "log/baseline.log"},
  {"scenario": {"Mutant": {"name": "a.rs:1:1: caught one"}}, "summary": "CaughtMutant", "log_path": "log/caught.log"},
  {"scenario": {"Mutant": {"name": "a.rs:2:1: missed one"}}, "summary": "MissedMutant", "log_path": "log/missed.log"},
  {"scenario": {"Mutant": {"name": "a.rs:3:1: timed out"}}, "summary": "Timeout", "log_path": "log/timeout.log"},
  {"scenario": {"Mutant": {"name": "a.rs:4:1: unviable one"}}, "summary": "Unviable", "log_path": "log/unviable.log"},
  {"scenario": {"Mutant": {"name": "a.rs:5:1: failed"}}, "summary": "Failure", "log_path": "log/failure.log"},
  {"scenario": {"Mutant": {"name": "a.rs:6:1: escapes"}}, "summary": "MissedMutant", "log_path": "../outside.log"}
 ]}
JSON
{
  head -c 6000000 /dev/zero | tr '\0' 'x'
  echo
  echo "the debug log's end"
} >"$out/debug.log"

# Everything the summary reads, and the logs worth reading: the baseline's, the missed, timed
# out, and failed mutants'. Not the caught or unviable ones', the diffs, or a path outside.
results=$tmp/results/shard-0
said=$(GITHUB_STEP_SUMMARY=$tmp/summary.md bash "$keep" "$out" "$results")
for file in caught.txt missed.txt timeout.txt unviable.txt mutants.json outcomes.json shard threads; do
  cmp -s "$out/$file" "$results/$file" || fail "$file wasn't kept as it was"
done
for log in baseline missed timeout failure; do
  cmp -s "$out/log/$log.log" "$results/log/$log.log" || fail "the $log log wasn't kept"
done
for log in caught unviable; do
  [ ! -e "$results/log/$log.log" ] || fail "the $log log was kept"
done
[ ! -e "$results/diff" ] || fail "the diffs were kept"
[ -z "$(find "$tmp/results" -name outside.log)" ] || fail "a log_path outside mutants.out was followed"
pass

# debug.log, its last 5 MB.
size=$(wc -c <"$results/debug.log" | tr -d ' ')
[ "$size" -eq 5000000 ] || fail "debug.log kept at $size bytes, not 5000000"
[ "$(tail -n 1 "$results/debug.log")" = "the debug log's end" ] || fail "debug.log's end wasn't kept"
pass

# The counts said, and added to the job's summary.
case $said in
*"1 caught, 1 missed, 1 timed out, 1 unviable"*) ;;
*) fail "it said \"$said\"" ;;
esac
grep -q "1 caught, 1 missed, 1 timed out, 1 unviable" "$tmp/summary.md" || fail "the counts weren't added to the job's summary"
pass

# Whole, it's marked `kept`, the run's attempt in it.
[ "$(cat "$results/kept")" = 1 ] || fail "no kept marker, or not attempt 1"
pass

# Kept again for a re-run, into the same directory: what was there is replaced, not added
# to, a log no longer missed is gone, and the marker is the new attempt's.
printf 'a.rs:1:1: caught one\na.rs:2:1: missed one\n' >"$out/caught.txt"
: >"$out/missed.txt"
cp "$out/outcomes.json" "$tmp/outcomes.json"
sed 's/"MissedMutant", "log_path": "log\/missed.log"/"CaughtMutant", "log_path": "log\/missed.log"/' "$tmp/outcomes.json" >"$out/outcomes.json"
GITHUB_RUN_ATTEMPT=2 bash "$keep" "$out" "$results" >/dev/null
[ "$(grep -c . "$results/caught.txt")" -eq 2 ] && [ ! -s "$results/missed.txt" ] || fail "a re-run's lists weren't kept as they are"
[ ! -e "$results/log/missed.log" ] || fail "a re-run kept the log of a mutant it caught"
[ "$(cat "$results/kept")" = 2 ] || fail "a re-run's marker isn't attempt 2"
pass

# A shard that stopped before cargo-mutants wrote anything: its results directory holds its
# marker alone, and nothing fails.
bash "$keep" "$tmp/out/none" "$tmp/results/early" >/dev/null || fail "a missing mutants.out failed it"
[ "$(ls -A "$tmp/results/early")" = kept ] || fail "a missing mutants.out: not the marker alone"
pass

# A mutants.out with no outcomes.json yet (stopped mid-baseline: cargo-mutants writes it only
# once the baseline ends): its lists kept, and the baseline's log so far, and nothing fails.
partial=$tmp/partial/mutants.out
mkdir -p "$partial/log"
echo '[]' >"$partial/mutants.json"
echo "the baseline so far" >"$partial/log/baseline.log"
echo "a mutant's log" >"$partial/log/other.log"
bash "$keep" "$partial" "$tmp/results/partial" >/dev/null || fail "a missing outcomes.json failed it"
[ -f "$tmp/results/partial/mutants.json" ] || fail "a missing outcomes.json: the lists weren't kept"
cmp -s "$partial/log/baseline.log" "$tmp/results/partial/log/baseline.log" || fail "a missing outcomes.json: the baseline's log wasn't kept"
[ ! -e "$tmp/results/partial/log/other.log" ] || fail "a missing outcomes.json: another log was kept"
pass

# An outcomes.json that isn't JSON: the lists kept, it says so, and nothing fails.
echo "not json" >"$partial/outcomes.json"
said=$(bash "$keep" "$partial" "$tmp/results/bad") || fail "an unreadable outcomes.json failed it"
case $said in *"couldn't be read"*) ;; *) fail "an unreadable outcomes.json: it said \"$said\"" ;; esac
[ -f "$tmp/results/bad/mutants.json" ] || fail "an unreadable outcomes.json: the lists weren't kept"
pass

# No directory given: refused, nothing removed. (The root is refused the same way, but never
# tried here: were the guard ever lost, the test itself would remove it.)
if bash "$keep" "$out" "" >/dev/null 2>&1; then fail "an empty results directory was taken"; fi
[ -f "$out/caught.txt" ] || fail "an empty results directory: mutants.out was touched"
pass

# Run as a re-run of the gate runs it, the job's attempt and summary set: it passes, and its
# cases leave that summary as it was. (Once, from the outer run only.)
if [ -z "$inner" ]; then
  echo "the gate's own summary" >"$tmp/gate-summary.md"
  GITHUB_RUN_ATTEMPT=2 GITHUB_STEP_SUMMARY=$tmp/gate-summary.md \
    "$BASH" "${BASH_SOURCE[0]}" --inner >"$tmp/inner.log" 2>&1 || fail "run as a gate's re-run (attempt 2), it failed: $(cat "$tmp/inner.log")"
  grep -q "cases passed" "$tmp/inner.log" || fail "run as a gate's re-run, it ran no cases: $(cat "$tmp/inner.log")"
  [ "$(cat "$tmp/gate-summary.md")" = "the gate's own summary" ] || fail "run as a gate's job, it wrote the job's summary"
  pass
fi

echo "mutants-keep: $passed cases passed"
