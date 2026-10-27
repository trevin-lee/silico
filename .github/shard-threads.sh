#!/usr/bin/env bash
# How many threads a real mutants shard takes (.github/workflows/mutants.yml): the CPUs idle
# as it starts, beyond a reserve for one gate, between LEAST and MOST. A shard's runner is at
# the idle CPU weight, so a gate always wins the CPU from it; the reserve leaves room for a
# gate that arrives later anyway, and LEAST is what a shard took before (a tenth of the VM).
# A shard keeps its count to the end, so on a quiet VM a long run's backlog takes the CPU
# that would otherwise sit idle (2026-10-27: 27 to 51% idle behind seven 3-thread shards).
#
# Idle is read as the filler reads it (.github/workflows/filler.yml): /proc/stat over a few
# seconds, idle and iowait time as a share of every CPU's. Shards that start together (a long
# run's seven) would each see the same idle CPUs, so they take turns under a lock, and each
# leaves a claim for its count that the next ones subtract until the shard has had time to
# load the CPU itself. Claims are files under the runners' shared home; old ones are deleted.
#
# Usage: bash .github/shard-threads.sh LEAST MOST   (prints the count and how it was chosen,
#        the count alone on the first line)
set -euo pipefail

least=$1
most=$2
cpus=$(nproc)
reserve=$((cpus / 4))
# Long enough for a shard's build to be compiling on every thread it took.
fresh_minutes=3
claims=$HOME/.cache/silico/shard-threads
mkdir -p "$claims"

exec 9>"$claims/lock"
flock 9

cpu() { awk '/^cpu / { print $2 + $3 + $4 + $5 + $6 + $7 + $8 + $9, $5 + $6 }' /proc/stat; }
read -r total0 idle0 < <(cpu)
sleep 5
read -r total1 idle1 < <(cpu)
idle=$((cpus * (idle1 - idle0) / (total1 - total0 > 0 ? total1 - total0 : 1)))

find "$claims" -name 'claim.*' -mmin +"$fresh_minutes" -delete
claimed=$(find "$claims" -name 'claim.*' -exec cat {} + | awk '{ sum += $1 } END { print sum + 0 }')

spare=$((idle - reserve - claimed))
threads=$((spare < least ? least : spare > most ? most : spare))
claim=$(mktemp "$claims/claim.XXXXXX")
echo "$threads" >"$claim"

echo "$threads"
echo "$threads threads: $idle of $cpus CPUs idle, $reserve kept for a gate, $claimed claimed by shards starting (between $least and $most)"
