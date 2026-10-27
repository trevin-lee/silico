#!/usr/bin/env bash
# How many threads a real mutants shard takes (.github/workflows/mutants.yml): the CPUs idle
# as it starts, beyond a reserve for one gate and what the other shards running hold, between
# LEAST and MOST. A shard's runner is at the idle CPU weight, so a gate always wins the CPU
# from it; the reserve leaves room for a gate that arrives later anyway, and LEAST is what a
# shard took before (a tenth of the VM). A shard keeps its count to the end, so on a quiet VM
# a long run's backlog takes the CPU that would otherwise sit idle (2026-10-27: 27 to 51%
# idle behind seven 3-thread shards).
#
# Idle is read as the filler reads it (.github/workflows/filler.yml): /proc/stat over a few
# seconds, idle and iowait time as a share of every CPU's. A moment's idle overstates what's
# spare (each mutant's rebuild is mostly serial, its tests use every thread), so each shard
# holds a claim for its count, a file named for its runner, until its job ends (`release`),
# and a shard takes no more than the CPUs no claim and no gate's reserve holds (23 of 30),
# whatever their phases, and LEAST once those are held: at most 32 threads for a long run's
# seven shards, 38 with the merge lane's two, starting in turn on an idle VM (27 before).
# Shards take turns under a lock, so a long run's seven starting together see each other's
# claims. A runner runs one job at a time, so a claim its job left (a runner that died) is
# replaced by its next shard; claims older than a shard's longest job (a day) are deleted.
#
# Usage: bash .github/shard-threads.sh LEAST MOST   (prints the count and how it was chosen,
#                                                    the count alone on the first line)
#        bash .github/shard-threads.sh release      (at the job's end, `if: always()`)
set -euo pipefail

claims=$HOME/.cache/silico/shard-threads
mkdir -p "$claims"
claim=$claims/claim.$RUNNER_NAME

# Under the lock even to release, so a shard choosing never reads a claim half gone.
exec 9>"$claims/lock"
flock 9

if [ "$1" = release ]; then
  rm -f "$claim"
  exit 0
fi

least=$1
most=$2
cpus=$(nproc)
reserve=$((cpus / 4))

cpu() { awk '/^cpu / { print $2 + $3 + $4 + $5 + $6 + $7 + $8 + $9, $5 + $6 }' /proc/stat; }
read -r total0 idle0 < <(cpu)
sleep 5
read -r total1 idle1 < <(cpu)
idle=$((cpus * (idle1 - idle0) / (total1 - total0 > 0 ? total1 - total0 : 1)))

rm -f "$claim"
find "$claims" -name 'claim.*' -mmin +1440 -delete
claimed=$(find "$claims" -name 'claim.*' -exec cat {} + | awk '{ sum += $1 } END { print sum + 0 }')

unheld=$((cpus - reserve - claimed))
spare=$((idle - reserve < unheld ? idle - reserve : unheld))
threads=$((spare < least ? least : spare > most ? most : spare))
echo "$threads" >"$claim"

echo "$threads"
echo "$threads threads: $idle of $cpus CPUs idle, $reserve kept for a gate, $claimed held by other shards (between $least and $most)"
