#!/usr/bin/env bash
# .github/mutants-tests.py, the tests a mutants run gives each mutant when its `tests` input
# is empty, against a made-up workspace shaped as Silico's is: a protocol every package uses,
# silico-worker and the worker packages built on it (kernel, render), silico-core, which runs
# the workers from disk without depending on them, and mcp, cli and app above it, the kernel
# taking core as a dev-dependency. Each case says which tests it expects, worked by hand from
# that graph. Needs bash and python3; no network. `pixi run test` runs it.
#
#   pixi run test-ci-decisions   (or bash tests/mutants-tests-test.sh)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/silico-mutants-tests.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

fail() {
  echo "mutants-tests: $*" >&2
  exit 1
}

# The workspace: each package, its directory, and its dependencies (name:kind, kind normal,
# dev, or build).
python3 -I - "$tmp/metadata.json" <<'PYEOF'
import json, sys
root = "/w"
graph = {
    "silico-protocol": [],
    "silico-worker": ["silico-protocol:normal"],
    "silico-kernel": ["silico-protocol:normal", "silico-worker:normal", "silico-core:dev"],
    "silico-render": ["silico-protocol:normal", "silico-worker:normal"],
    "silico-core": ["silico-protocol:normal", "silico-core:dev"],
    "silico-mcp": ["silico-core:normal"],
    "silico-cli": ["silico-core:normal", "silico-mcp:normal"],
    "silico-app": ["silico-core:normal", "silico-render:normal", "silico-font:normal"],
    "silico-font": [],
    "silico-gen": [],
    "silico-uses-gen": ["silico-gen:build"],
    "silico-top": ["silico-uses-gen:normal"],
}
packages = []
for name, deps in graph.items():
    packages.append({
        "name": name,
        "id": f"path+file://{root}/crates/{name}#0.1.0",
        "manifest_path": f"{root}/crates/{name}/Cargo.toml",
        "dependencies": [
            {"name": d.split(":")[0], "kind": None if d.split(":")[1] == "normal" else d.split(":")[1]}
            for d in deps
        ] + [{"name": "serde", "kind": None}],
    })
packages.append({"name": "serde", "id": "registry+serde#1.0.0", "manifest_path": "/registry/serde/Cargo.toml", "dependencies": []})
json.dump({
    "workspace_root": root,
    "workspace_members": [p["id"] for p in packages if p["name"] != "serde"],
    "packages": packages,
}, open(sys.argv[1], "w"))
PYEOF

passed=0
# expect NAME WANT_TESTS WANT_WHY_PART MUTANT...: the tests and a part of the reason for
# mutants named as `cargo mutants --list` names them.
expect() {
  local name=$1 want=$2 why=$3 got_tests got_why
  shift 3
  printf '%s\n' "$@" >"$tmp/mutants.txt"
  {
    read -r got_tests
    read -r got_why
  } < <(python3 -I "$repo/.github/mutants-tests.py" "$tmp/metadata.json" "$tmp/mutants.txt")
  [ "$got_tests" = "$want" ] || fail "$name: tests \"$got_tests\", not \"$want\""
  case $got_why in
  *"$why"*) ;;
  *) fail "$name: said \"$got_why\", not \"$why\"" ;;
  esac
  passed=$((passed + 1))
}

m() { echo "crates/$1/src/lib.rs:10:5: replace f -> bool with true"; }

# silico-mcp: only the CLI depends on it.
expect mcp "silico-cli silico-mcp" "the mutated packages (silico-mcp) and those that depend on them" "$(m silico-mcp)"
# silico-app: nothing depends on it.
expect app "silico-app" "(silico-app)" "$(m silico-app)"
# silico-font: the app depends on it.
expect font "silico-app silico-font" "(silico-font)" "$(m silico-font)"
# Two packages: the union of their dependents.
expect union "silico-app silico-cli silico-mcp" "(silico-app silico-mcp)" "$(m silico-mcp)" "$(m silico-app)"
# silico-core: its dependents, the kernel's dev-dependency on it among them. The render worker
# doesn't depend on core, and core is linked into no worker.
expect core "silico-app silico-cli silico-core silico-kernel silico-mcp" "(silico-core)" "$(m silico-core)"
# A build-dependency counts: the dependent's build script runs the mutated code; and so does
# what depends on the dependent, at any depth.
expect build "silico-gen silico-top silico-uses-gen" "(silico-gen)" "$(m silico-gen)"
# A worker package, silico-worker itself, and what they're built from: every workspace test.
expect kernel "" "silico-kernel is linked into the worker binaries" "$(m silico-kernel)"
expect worker "" "silico-worker is linked into the worker binaries" "$(m silico-worker)"
expect protocol "" "silico-protocol is linked into the worker binaries" "$(m silico-protocol)"
# One mutant linked into a worker among others: every workspace test, naming it.
expect mixed "" "silico-render is linked" "$(m silico-app)" "$(m silico-render)"
# A file in a crate's tests or benches is still its package's.
expect nested "silico-cli silico-mcp" "(silico-mcp)" "crates/silico-mcp/src/tools/words.rs:3:1: delete !"
# No mutants, or one in no package: every workspace test.
expect none "" "no mutants"
expect stray "" "a mutant in tools/x.rs is in no package of the workspace" "tools/x.rs:1:1: replace g with ()"
# A package whose name another's begins with isn't taken for it (silico-core vs silico-corex).
expect prefix "" "a mutant in crates/silico-corex/src/lib.rs is in no package" "$(m silico-corex)"

echo "mutants-tests: $passed cases passed"
