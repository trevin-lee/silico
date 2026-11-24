#!/usr/bin/env bash
# Room on a GitHub-hosted runner's disk for a gate's build: its pixi environments (FreeCAD's
# among them), OCCT, the toolchain, and a debug build of the workspace (about 12 GiB) with the
# cache's archive of it beside, some 30 GB in all. The ubuntu-24.04 image has had as little as
# 14 GB free on its root, and as much as 89 GB (2026-11-24, a 145 GB disk): with 40 GB free
# already, nothing is removed, since removing takes over a minute. Otherwise the image's SDKs
# and toolchains Silico never uses (pixi and rustup bring its own) go, removed side by side,
# and docker's images too. Prints the disk before and after.
#
# Usage: bash .github/free-disk.sh   (a step before anything that fills the disk)
set -euo pipefail

# The machine, for reading its times: hosted runners' CPUs differ from job to job.
echo "$(nproc) CPUs: $(lscpu | sed -n 's/^Model name: *//p'); $(free -g | awk '/^Mem:/ { print $2 }') GiB of memory"
df -h / /mnt 2>/dev/null || df -h /
free_gib=$(( $(df -Pk / | awk 'NR == 2 { print $4 }') / 1024 / 1024 ))
if [ "$free_gib" -ge 40 ]; then
  echo "$free_gib GiB free on /: nothing removed"
  exit 0
fi
unused=(
  /usr/local/lib/android
  /usr/share/dotnet
  /opt/ghc
  /usr/local/.ghcup
  /opt/hostedtoolcache/CodeQL
  /opt/hostedtoolcache/go
  /opt/hostedtoolcache/PyPy
  /opt/hostedtoolcache/Ruby
  /usr/local/share/boost
  /usr/local/share/powershell
  /usr/local/share/chromium
  /usr/local/lib/node_modules
  /usr/share/swift
  /opt/microsoft
  /opt/google
  /opt/az
  /usr/lib/jvm
)
pids=()
for dir in "${unused[@]}"; do
  if [ -e "$dir" ]; then
    sudo rm -rf "$dir" &
    pids+=($!)
  fi
done
if command -v docker >/dev/null 2>&1; then
  sudo docker image prune --all --force >/dev/null 2>&1 &
  pids+=($!)
fi
status=0
for pid in "${pids[@]}"; do wait "$pid" || status=1; done
if [ "$status" -ne 0 ]; then echo "::warning::some of the image's unused tools couldn't be removed"; fi
df -h / /mnt 2>/dev/null || df -h /
