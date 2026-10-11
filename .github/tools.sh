#!/usr/bin/env bash
# Installs what CI needs that pixi doesn't provide, into the runner's home, once: rustup
# (rust-toolchain.toml picks the toolchain), pixi, and cargo-nextest. Puts them on the
# job's PATH.
set -euo pipefail

PIXI_VERSION=v0.81.0
NEXTEST_VERSION=0.9.146

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
fi
echo "$HOME/.cargo/bin" >>"$GITHUB_PATH"

if [ "$("$HOME/.pixi/bin/pixi" --version 2>/dev/null)" != "pixi ${PIXI_VERSION#v}" ]; then
  curl -fsSL https://pixi.sh/install.sh |
    PIXI_VERSION=$PIXI_VERSION PIXI_NO_PATH_UPDATE=1 bash
fi
echo "$HOME/.pixi/bin" >>"$GITHUB_PATH"

# It prints "cargo-nextest 0.9.146 (<commit> <date>)".
case "$("$HOME/.cargo/bin/cargo-nextest" nextest --version 2>/dev/null | head -1)" in
"cargo-nextest $NEXTEST_VERSION "*) ;;
*) curl -LsSf "https://get.nexte.st/$NEXTEST_VERSION/linux" | tar zxf - -C "$HOME/.cargo/bin" ;;
esac
