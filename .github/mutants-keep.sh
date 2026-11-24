#!/usr/bin/env bash
# A mutants shard's outcomes kept for the run's summary (mutants.yml), which reads them from
# the shard's artifact (mutants-shard-<n>): only what the summary and triage read, not the
# logs of every mutant, which the shard uploads apart.
#
#   bash .github/mutants-keep.sh MUTANTS_OUT RESULTS
#
# From cargo-mutants' MUTANTS_OUT (mutants.out, possibly partial or missing: a stopped
# shard's), into RESULTS: the outcome lists (caught, missed, timeout, unviable), the mutants
# it planned (mutants.json), outcomes.json, the shard's markers (shard, threads, and none,
# when it found no mutants: .github/mutants-none.sh), the
# baseline's log (whenever it's there: cargo-mutants writes outcomes.json only once the
# baseline ends) and those of each mutant missed, timed out or failed (each with its path
# under log/), and the last 5 MB of debug.log. Never the logs of mutants caught or
# unviable, which are most of mutants.out and which nobody reads. What RESULTS held before
# is replaced (a re-run of the shard), and `kept` is written last, so the summary takes a
# copy only once it's whole. Prints the shard's counts, and adds them to the job's summary
# when there is one.
set -uo pipefail

out=$1
results=$2
case $results in
"" | /) echo "mutants-keep: no results directory given" >&2; exit 2 ;;
esac
rm -rf "$results"
mkdir -p "$results"
if [ ! -d "$out" ]; then
  echo "mutants-keep: $out isn't there: the shard stopped before cargo-mutants wrote anything"
  echo "${GITHUB_RUN_ATTEMPT:-1}" >"$results/kept"
  exit 0
fi
for file in caught.txt missed.txt timeout.txt unviable.txt mutants.json outcomes.json shard threads none; do
  if [ -f "$out/$file" ]; then cp "$out/$file" "$results/"; fi
done
if [ -f "$out/debug.log" ]; then tail -c 5000000 "$out/debug.log" >"$results/debug.log"; fi
if [ -f "$out/log/baseline.log" ]; then
  mkdir -p "$results/log"
  cp "$out/log/baseline.log" "$results/log/"
fi
# The other logs worth reading, by the outcomes cargo-mutants wrote (each outcome's log_path,
# under mutants.out): each missed, timed out, or failed mutant's.
if [ -f "$out/outcomes.json" ]; then
  python3 -I - "$out" "$results" <<'PYEOF' || echo "mutants-keep: the logs couldn't be kept"
import json, os, shutil, sys
out, results = sys.argv[1], sys.argv[2]
try:
    with open(os.path.join(out, "outcomes.json")) as f:
        outcomes = json.load(f).get("outcomes", [])
except (OSError, ValueError, AttributeError) as error:
    print(f"mutants-keep: outcomes.json couldn't be read ({error}); no logs kept")
    sys.exit(0)
for outcome in outcomes:
    log = outcome.get("log_path")
    if not log:
        continue
    if outcome.get("summary") not in ("MissedMutant", "Timeout", "Failure"):
        continue
    source = os.path.normpath(os.path.join(out, log))
    if not source.startswith(os.path.normpath(out) + os.sep) or not os.path.isfile(source):
        continue
    target = os.path.join(results, os.path.relpath(source, out))
    os.makedirs(os.path.dirname(target), exist_ok=True)
    shutil.copyfile(source, target)
PYEOF
fi
count() {
  if [ -f "$results/$1.txt" ]; then grep -c . "$results/$1.txt" || true; else echo 0; fi
}
echo "${GITHUB_RUN_ATTEMPT:-1}" >"$results/kept"
line="Kept for the summary: $(count caught) caught, $(count missed) missed, $(count timeout) timed out, $(count unviable) unviable, in $results"
echo "$line"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "$line" >>"$GITHUB_STEP_SUMMARY"; fi
