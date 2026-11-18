#!/bin/bash
# usage: w3-load2.sh BURNERS TEST...: each app test run alone by cargo test (no nextest deadline), on two CPUs with N burners.
n=$1; shift
pids=()
for i in $(seq $n); do taskset -c 0,1 bash -c 'while :; do :; done' & pids+=($!); done
trap 'kill ${pids[@]} 2>/dev/null' EXIT
for t in "$@"; do
  start=$(date +%s)
  taskset -c 0,1 pixi run -- cargo test -q -p silico-app --test ${TESTBIN:-app} -- "$t" --exact --nocapture 2>&1 | grep -E "test result|PollError|panicked|timed out|render wait|lavapipe didn" | sort | uniq -c | sort -rn | head -30
  echo "$t: $(( $(date +%s) - start ))s with $n burners"
done
