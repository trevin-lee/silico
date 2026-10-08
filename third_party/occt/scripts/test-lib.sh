# Sourced by the guard tests (test-guards.sh, test-cache.sh): a temporary directory and
# the expect_* helpers. Each test prints "ok: ..." or reports "GUARD BROKEN: ..."; call
# finish_guards at the end.
tmp=$(mktemp -d)
# chmod: some fixtures are unreadable on purpose.
trap 'chmod -R u+rwx "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

failures=0
guard_broken() {
  echo "GUARD BROKEN: $1" >&2
  failures=$((failures + 1))
}
expect_fail() {
  local name=$1
  shift
  if "$@" >"$tmp/out.log" 2>&1; then
    guard_broken "$name passed on bad input"
  else
    echo "ok: $name rejects bad input"
  fi
}
# Like expect_fail, but the failure must also print <pattern>, so a guard that fails for
# an unrelated reason (a crash, a missing file) doesn't count.
expect_fail_with() {
  local name=$1 pattern=$2
  shift 2
  if "$@" >"$tmp/out.log" 2>&1; then
    guard_broken "$name passed on bad input"
  elif grep -q "$pattern" "$tmp/out.log"; then
    echo "ok: $name rejects bad input"
  else
    cat "$tmp/out.log" >&2
    guard_broken "$name failed, but not with \"$pattern\" (output above)"
  fi
}
expect_pass() {
  local name=$1
  shift
  if "$@" >"$tmp/out.log" 2>&1; then
    echo "ok: $name accepts good input"
  else
    cat "$tmp/out.log" >&2
    guard_broken "$name failed on good input (output above)"
  fi
}
finish_guards() { # <suite name>
  if [ "$failures" -ne 0 ]; then
    echo "$1: $failures guard(s) broken" >&2
    exit 1
  fi
  echo "$1: every guard rejects bad input and accepts good input"
}
