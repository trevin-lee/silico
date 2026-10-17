#!/usr/bin/env bash
# The gate (docs/roadmap/m0-foundations.md, M0.3): every check, on the committed tree,
# before a merge. Uncommitted changes aren't checked, so a branch can't pass because of a
# file that only exists in someone's working tree. It runs in the worktree that
# tests/committed-tree.sh keeps, one per checkout.
#
# Run through pixi:  pixi run gate
set -euo pipefail

# shellcheck source=tests/committed-tree.sh
source "$(dirname "${BASH_SOURCE[0]}")/committed-tree.sh"

step() { echo "gate: $*"; }

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

step "test"
pixi run test

step "determinism: one stress run"
STRESS_RUNS=1 pixi run stress

echo "gate: passed ($commit)"
