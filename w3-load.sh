#!/bin/bash
# usage: w3-load.sh BURNERS FILTER THREADS: the app's tests matching FILTER on two CPUs, N burners on them.
n=${1:-8}; filter=${2:-lavapipe}; threads=${3:-4}
pids=()
for i in $(seq $n); do taskset -c 0,1 bash -c 'while :; do :; done' & pids+=($!); done
trap 'kill ${pids[@]} 2>/dev/null' EXIT
start=$(date +%s)
taskset -c 0,1 pixi run -- cargo nextest run -p silico-app --no-fail-fast --test-threads $threads -E "test(/$filter/)" 2>&1 | grep -E "^ +(FAIL|PASS|TIMEOUT|SIGTERM)|Summary|PollError|timed out|panicked" | head -40
echo "elapsed $(( $(date +%s) - start ))s with $n burners"
