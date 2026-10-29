#!/usr/bin/env bash
# How many threads a real mutants shard takes (.github/workflows/mutants.yml): the CPUs idle as
# it starts, beyond a reserve for one gate, between LEAST and MOST, but never so many that the
# shards' claims could pass 28 (two short of the VM) once every runner a shard could still start
# on has taken 2. A shard's runner is at the idle CPU weight, so a gate always wins the CPU from
# it; the reserve leaves room for a gate that arrives later anyway, and LEAST is what a shard
# took before (a tenth of the VM), or 2 where that would pass the bound. A shard keeps its count
# to the end. The bound is exact, and costs a long run on a quiet VM speed (in practice the
# first shard takes 3 or 4, by the idle CPUs, and every later one 2: 16 threads for a long run
# alone, not #221's 32, 2026-10-27): filler, at the idle weight, takes the CPU it leaves (Main,
# 2026-10-28). Exact while every claim is this rule's: a shard run at its caller's fallback (no
# claim) or a claim its runner leaked can pass it by a few threads, until the shard ends or the
# claim ages out. Past 28, the shards' tests held to a wall clock time out under load, and one
# that does counts its mutant as caught.
#
# Idle is read as the filler reads it (.github/workflows/filler.yml): /proc/stat over a few
# seconds, idle and iowait time as a share of every CPU's; and what filler's own runners used
# meanwhile counts as idle too (below). Each shard holds a claim for its count, a file named
# for its runner instance, until its job ends (`release`). Shards take turns under a lock, so
# a long run's seven starting together see each other's claims. A runner instance runs one job
# at a time, so a claim its job left (a runner that died) is replaced by that instance's next
# shard; claims older than a shard's longest job (a day) are deleted.
#
# Usage: bash .github/shard-threads.sh LEAST MOST   (prints the count and how it was chosen,
#                                                    the count alone on the first line)
#        bash .github/shard-threads.sh release      (at the job's end, `if: always()`)
#        SILICO_SHARD_IDLE=N to assume N idle CPUs instead of sampling them (for trying the
#        rule by hand with claims faked under a scratch HOME, as it was tried on the VM).
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
# it uses counts as spare, not as busy (#249): it decides only whether the first shard takes 3
# or 4 now that the bound sets the rest.
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

# No shard's choice may take the claims past the ceiling, two CPUs short of the VM, counting 2
# for every runner a real shard could still start on: thirteen of them (silico-5 to -12, -15 to
# -17, -20, -21, as /etc/ci-runner/labels gives `mutants` and `mutants-pr`; .github/workflows/
# filler.yml lists the same lanes). So the total never passes 28, whatever starts later, and no
# shard waits. On a quiet VM a long run takes less than it could (its first shards about 4, the
# rest 2: about 16 to 20 threads, not #221's 32); filler, at the idle weight on silico-5 to -12,
# -22 and -23, takes what's left. The shards' tests held to a wall clock were measured at 24 to
# 28 threads (2026-10-27); past that, one timed out counts its mutant as caught, which no run
# shows (Main, 2026-10-28: an exact bound over long runs' speed).
ceiling=$((cpus - 2))
runners=13
floor=2
# Tests only: the idle CPUs to assume instead of sampling them.
if [ -n "${SILICO_SHARD_IDLE:-}" ]; then
  sample() { idle=$SILICO_SHARD_IDLE; fillers=0; }
fi
sample
rm -f "$claim"
find "$claims" -name 'claim.*' -mmin +1440 -delete
claimed=$(find "$claims" -name 'claim.*' -exec cat {} + | awk '{ sum += $1 } END { print sum + 0 }')
claiming=$(find "$claims" -name 'claim.*' | wc -l)
# The runners that could still start a shard after this one, each kept 2.
later=$((runners - 1 - claiming > 0 ? runners - 1 - claiming : 0))
allowed=$((ceiling - claimed - floor * later))
spare=$((idle - reserve))
threads=$((spare < most ? spare : most))
threads=$((threads < allowed ? threads : allowed))
threads=$((threads < least ? least : threads))
threads=$((threads > allowed ? allowed : threads))
threads=$((threads < floor ? floor : threads))
echo "$threads" >"$claim"

echo "$threads"
echo "$threads threads: $idle of $cpus CPUs idle ($fillers of them filler's), $reserve kept for a gate, $claimed held by $claiming other shards, $allowed allowed with 2 kept for each of $later more (between $floor and $most)"
