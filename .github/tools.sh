#!/usr/bin/env bash
# Installs what CI needs that pixi doesn't provide, into the runner's home, once: rustup
# (rust-toolchain.toml picks the toolchain), pixi, cargo-nextest, and cargo-mutants. Puts
# them on the job's PATH.
set -euo pipefail

PIXI_VERSION=v0.81.0
NEXTEST_VERSION=0.9.146
MUTANTS_VERSION=27.1.0
MUTANTS_SHA256=dfe6dc37d0342c891d2829b5a695aa57c2d0edecef7e7d0399a30cc6e206411e

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

# Verified, and moved into place whole: the mutants workflow starts several jobs on this
# machine at once, and one may run it while another is installing it.
if [ "$("$HOME/.cargo/bin/cargo-mutants" mutants --version 2>/dev/null)" != "cargo-mutants $MUTANTS_VERSION" ]; then
  download=$(mktemp -d "$HOME/.cargo/bin/.cargo-mutants.XXXXXX")
  curl -fsSL -o "$download/cargo-mutants.tar.gz" \
    "https://github.com/sourcefrog/cargo-mutants/releases/download/v$MUTANTS_VERSION/cargo-mutants-x86_64-unknown-linux-gnu.tar.gz"
  echo "$MUTANTS_SHA256  $download/cargo-mutants.tar.gz" | sha256sum -c --quiet
  tar zxf "$download/cargo-mutants.tar.gz" -C "$download" cargo-mutants
  mv -f "$download/cargo-mutants" "$HOME/.cargo/bin/cargo-mutants"
  rm -rf "$download"
fi
