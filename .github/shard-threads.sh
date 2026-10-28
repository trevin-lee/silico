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
# seconds, idle and iowait time as a share of every CPU's; and what filler's own runners used
# meanwhile counts as idle too (below). A moment's idle overstates what's spare (each mutant's
# rebuild is mostly serial, its tests use every thread), so each shard holds a claim for its
# count, a file named for its runner instance, until its job ends (`release`), and a shard
# takes no more than the CPUs no claim and no gate's reserve holds (23 of 30), whatever their
# phases, and at least LEAST while claims are short of those 23, 2 once they reach them, and
# none past 28 (below). Shards take turns under a lock, so a long run's seven starting
# together see each other's claims. A runner instance runs one job at a time, so a claim its
# job left (a runner that died) is replaced by that instance's next shard; claims older than a
# shard's longest job (a day) are deleted.
#
# Usage: bash .github/shard-threads.sh LEAST MOST   (prints the count and how it was chosen,
#                                                    the count alone on the first line)
#        bash .github/shard-threads.sh release      (at the job's end, `if: always()`)
set -euo pipefail

claims=$HOME/.cache/silico/shard-threads
mkdir -p "$claims"
# By instance (pve-silico-9), not by registration (pve-silico-9-1793152973): each job's runner
# registers again under a new suffix, so only the instance's name is the same for its next shard.
instance=$RUNNER_NAME
# The suffix is the registration's epoch time (/usr/local/libexec/ci-runner), nine digits or more:
# a name without one (pve-silico-10) stays whole.
if [[ $RUNNER_NAME =~ ^(.*)-[0-9]{9,}$ ]]; then instance=${BASH_REMATCH[1]}; fi
claim=$claims/claim.$instance

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
# The CPU time, in microseconds, the runners only filler uses (silico-22, -23) have taken: their
# services' cgroups. Filler is background work a shard's whole run outlasts (an hour for
# mutants, two for a sweep, cut off for any real run that starts), and holds no claim, so what
# it uses counts as spare, not as busy: else a long run's queued shards, starting beside it,
# would take LEAST for hours.
filler() {
  # Neither there (another VM, a runner renamed): nothing, not a failed pipeline.
  { cat /sys/fs/cgroup/system.slice/system-ci\\x2drunner.slice/ci-runner@silico-2[23].service/cpu.stat \
    2>/dev/null || true; } | awk '/^usage_usec / { sum += $2 } END { print sum + 0 }'
}
# The CPUs idle over the next 5 s, filler's counted as idle (above): `idle`, `fillers`.
sample() {
  local total0 idle0 total1 idle1 filler0 filler1 ticks hz
  read -r total0 idle0 < <(cpu)
  filler0=$(filler)
  sleep 5
  read -r total1 idle1 < <(cpu)
  filler1=$(filler)
  ticks=$((total1 - total0 > 0 ? total1 - total0 : 1))
  # /proc/stat counts every CPU's time in ticks (USER_HZ, 100 a second), cgroups in
  # microseconds.
  hz=$(getconf CLK_TCK)
  fillers=$((cpus * (filler1 - filler0) * hz / 1000000 / ticks))
  idle=$((cpus * (idle1 - idle0) / ticks + fillers))
}

# Past the budget, a shard takes 2 threads, not LEAST; and none starts that would take the
# claims past the ceiling, two CPUs short of the VM: it waits for a claim to go, polling, up to
# a bound, then runs at 2 all the same, saying so. Thirteen merge-lane runners (2026-10-28) at
# LEAST each would claim 39 threads, against the 24 to 28 the shards' tests held to a wall clock
# were measured at; one of those timed out counts its mutant as caught, which no run shows.
budget=$((cpus - reserve))
ceiling=$((cpus - 2))
crowded=2
wait=${SILICO_SHARD_WAIT:-1200}
poll=${SILICO_SHARD_POLL:-30}
began=$(date +%s)
deadline=$((began + wait))
waited=0
idle=-
fillers=-
while :; do
  rm -f "$claim"
  find "$claims" -name 'claim.*' -mmin +1440 -delete
  claimed=$(find "$claims" -name 'claim.*' -exec cat {} + | awk '{ sum += $1 } END { print sum + 0 }')
  if [ "$claimed" -lt "$budget" ]; then
    # Only this choice reads the idle CPUs: a shard waiting holds the lock for no 5 s sample.
    sample
    unheld=$((budget - claimed))
    spare=$((idle - reserve < unheld ? idle - reserve : unheld))
    threads=$((spare < least ? least : spare > most ? most : spare))
    why="between $least and $most"
    break
  fi
  if [ $((claimed + crowded)) -le "$ceiling" ]; then
    threads=$crowded
    why="$claimed claimed reach the budget of $budget: $crowded"
    break
  fi
  if [ "$(date +%s)" -ge "$deadline" ]; then
    threads=$crowded
    why="past the ceiling of $ceiling: $crowded all the same"
    break
  fi
  # Give the lock back while waiting, so shards ending can release their claims. Said on stderr
  # once, so the job's log shows why its shard hasn't started (stdout is the caller's).
  if [ "$waited" -eq 0 ]; then
    echo "shard-threads: $claimed threads claimed, past the ceiling of $ceiling with 2 more: waiting up to ${wait}s for a claim to go" >&2
  fi
  flock -u 9
  sleep "$poll"
  flock 9
  waited=$(($(date +%s) - began))
done
echo "$threads" >"$claim"

echo "$threads"
if [ "$waited" -gt 0 ]; then why="$why, after waiting ${waited}s for claims to go"; fi
if [ "$idle" = - ]; then seen="idle not read"; else seen="$idle of $cpus CPUs idle ($fillers of them filler's)"; fi
echo "$threads threads: $seen, $reserve kept for a gate, $claimed held by other shards ($why)"
