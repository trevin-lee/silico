#!/usr/bin/env bash
# Room on a GitHub-hosted runner's disk for a gate's build: the ubuntu-24.04 image leaves about
# 14 GB free on its root, and a gate needs its pixi environments (FreeCAD's among them), OCCT,
# the toolchain, and a debug build of the workspace (about 12 GiB) with the cache's archive of
# it beside. The image's SDKs and toolchains Silico never uses (pixi and rustup bring its own)
# go, removed side by side, which takes about a minute; docker's images too. Prints the disk
# before and after.
#
# Usage: bash .github/free-disk.sh   (a step before anything that fills the disk)
set -euo pipefail

df -h / /mnt 2>/dev/null || df -h /
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
