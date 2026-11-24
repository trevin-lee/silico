#!/usr/bin/env bash
# The gate (docs/roadmap/m0-foundations.md, M0.3): every check, on the committed tree,
# before a merge. Uncommitted changes aren't checked, so a branch can't pass because of a
# file that only exists in someone's working tree. It runs in the worktree that
# tests/committed-tree.sh keeps, one per checkout.
#
# Run through pixi:  pixi run gate              (every part, in turn: what a merge needs)
#
# Or one part, as CI runs them side by side on several runners (.github/workflows/gate.yml),
# which together are every check the whole gate makes:
#   bash tests/gate.sh lint                     (the workspace lints, cargo-deny, fmt and clippy)
#   bash tests/gate.sh test [PARTITION]         (pixi run test; nextest's --partition, such as
#                                                count:2/3, runs only that share of the tests,
#                                                every share together being every test)
#   bash tests/gate.sh stress                   (one stress run)
set -euo pipefail

part=${1:-all}
partition=${2-}
case $part in
all | lint | stress)
  if [ $# -gt 1 ]; then
    echo "gate: only the test part takes a partition" >&2
    exit 2
  fi
  ;;
test)
  if [ $# -gt 2 ]; then
    echo "gate: the test part takes one partition" >&2
    exit 2
  fi
  if [ -n "$partition" ] && ! [[ $partition =~ ^(count|hash):([1-9][0-9]*)/([1-9][0-9]*)$ && ${BASH_REMATCH[2]} -le ${BASH_REMATCH[3]} ]]; then
    echo "gate: a partition is count:K/N or hash:K/N, with 1 <= K <= N, not '$partition'" >&2
    exit 2
  fi
  ;;
*)
  echo "usage: tests/gate.sh [lint | test [PARTITION] | stress]  (no part: every part)" >&2
  exit 2
  ;;
esac

# shellcheck source=tests/committed-tree.sh
source "$(dirname "${BASH_SOURCE[0]}")/committed-tree.sh"

step() { echo "gate: $*"; }

if [ "$part" = all ] || [ "$part" = lint ]; then
  step "every crate uses the workspace lints"
  for manifest in crates/*/Cargo.toml; do
    # [lints] followed by workspace = true, nothing between them.
    if ! awk '/^\[lints\]$/ { getline; if ($0 == "workspace = true") found = 1 } END { exit !found }' "$manifest"; then
      echo "gate: FAIL: $manifest doesn't have [lints] workspace = true" >&2
      exit 1
    fi
  done

  step "dependencies: banned crates, licenses, sources"
  pixi run --quiet cargo deny --log-level error check bans licenses sources

  step "lint"
  pixi run lint
fi

if [ "$part" = all ] || [ "$part" = test ]; then
  if [ -n "$partition" ]; then
    step "test (partition $partition)"
    pixi run test --partition "$partition"
  else
    step "test"
    pixi run test
  fi
fi

if [ "$part" = all ] || [ "$part" = stress ]; then
  step "determinism: one stress run"
  STRESS_RUNS=1 pixi run stress
fi

if [ "$part" = all ]; then
  echo "gate: passed ($commit)"
else
  echo "gate: $part${partition:+ ($partition)} passed ($commit)"
fi
