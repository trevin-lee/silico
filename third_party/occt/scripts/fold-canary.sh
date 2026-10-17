#!/usr/bin/env bash
# Fails if clang folds math calls with constant arguments under the given flags.
# Folding computes the result with the build machine's math library, bypassing the
# shim and leaving no import for check-math-imports.sh to see. The calls are generated
# from inexact-math.txt, the same list build.sh turns into -fno-builtin flags.
# Usage: fold-canary.sh <compiler flags...>
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

src=$tmp/canary.cpp
want=""
{
  echo '#include <cmath>'
  echo 'static inline double tenpow(int n) { return std::pow(10.0, -n); }'
  while read -r name arity flag; do
    case "$name" in '' | '#'*) continue ;; esac
    [ "${flag:-}" = nocanary ] && continue
    if [ "$name" = pow ]; then
      # pow(10, -n) after inlining: the pattern clang also turns into exp10.
      echo 'extern "C" double canary_pow() { return tenpow(20); }'
      echo 'extern "C" float canary_powf() { return std::pow(10.0f, -3.0f); }'
    elif [ "$arity" = 2 ]; then
      echo "extern \"C\" double canary_$name() { return std::$name(0.3, 0.7); }"
      echo "extern \"C\" float canary_${name}f() { return std::$name(0.3f, 0.7f); }"
    else
      echo "extern \"C\" double canary_$name() { return std::$name(0.37); }"
      echo "extern \"C\" float canary_${name}f() { return std::$name(0.37f); }"
    fi
    want="$want $name ${name}f"
  done < "$here/inexact-math.txt"
} > "$src"

clang++ -std=c++17 -O3 "$@" -c "$src" -o "$tmp/canary.o"
case "$(uname -s)" in
  Darwin) refs=$(xcrun nm -u "$tmp/canary.o" | sed 's/^_//') ;;
  *) refs=$(nm -u "$tmp/canary.o" | awk '{print $NF}') ;;
esac
folded=""
count=0
for f in $want; do
  count=$((count + 1))
  # Not `echo | grep -q`: grep stops at the first match, and under pipefail the echo it
  # leaves unread (SIGPIPE) would fail the test now and then.
  grep -qx "$f" <<<"$refs" || folded="$folded $f"
done
if [ "$count" -eq 0 ]; then
  echo "FAIL: no canary calls generated from inexact-math.txt" >&2
  exit 1
fi
if [ -n "$folded" ]; then
  echo "FAIL: clang folded these math calls at build time:$folded" >&2
  exit 1
fi
echo "OK: no math calls folded at build time ($count checked)"
