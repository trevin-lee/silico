#!/usr/bin/env bash
# Fails if any library exports a symbol defined by the libm shim. Each library keeps
# its own private copy, so nothing outside it binds to the shim by accident.
# Usage: check-shim-hidden.sh <shim object> <library>...
set -euo pipefail
shim=$1
shift
if [ $# -eq 0 ]; then
  echo "FAIL: no libraries given to check" >&2
  exit 1
fi
case "$(uname -s)" in
  Darwin)
    shim_syms=$(xcrun nm -gU "$shim" | awk '{print $NF}' | sort -u)
    exported() { xcrun nm -gU "$1" | awk '{print $NF}'; }
    ;;
  *)
    shim_syms=$(nm -g --defined-only "$shim" | awk '{print $NF}' | sort -u)
    exported() { nm -D --defined-only "$1" | awk '{print $NF}' | sed 's/@.*//'; }
    ;;
esac
[ -n "$shim_syms" ] || { echo "FAIL: no symbols read from $shim" >&2; exit 1; }
bad=""
for lib in "$@"; do
  leaked=$(comm -12 <(exported "$lib" | sort -u) <(echo "$shim_syms") | tr '\n' ' ')
  [ -z "$leaked" ] || bad="$bad\n  $(basename "$lib"): $leaked"
done
if [ -n "$bad" ]; then
  printf "FAIL: libraries export libm shim symbols:%b\n" "$bad" >&2
  exit 1
fi
echo "OK: no library exports libm shim symbols ($# checked)"
