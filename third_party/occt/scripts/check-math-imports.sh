#!/usr/bin/env bash
# Fails if any given shared library or executable imports a platform math function that
# isn't exact by IEEE 754 / C definition. Everything else must come from the libm shim.
# Arguments may be files or directories (every .dylib/.so in a directory is checked).
# Static archives and object files are rejected: they don't record where a symbol will
# come from, so check the linked library or executable instead.
# Usage: check-math-imports.sh <file or dir>...
set -euo pipefail

# Exact by definition, so every platform agrees.
exact='^(sqrt|fabs|floor|ceil|trunc|round|rint|nearbyint|lrint|llrint|lround|llround|fmod|remainder|remquo|ldexp|scalbn|scalbln|frexp|modf|nextafter|nexttoward|copysign|fmin|fmax|fdim|fma|ilogb|logb|isnan|isinf|finite)[fl]?$|^__(fpclassify|isnan|isinf|isfinite|signbit|inline_)|^fe(clear|raise|test|disable|get)except$|^fe(get|set)exceptflag$'
# Floating-point exception flag functions never compute a value, so they're exact. But
# turning traps on (feenableexcept) makes glibc raise SIGFPE where macOS ARM carries on,
# so it's allowed only in OCCT's TKernel, whose signal setup workers keep switched off.
trap_enable='^feenableexcept$'

files=()
for arg in "$@"; do
  if [ -d "$arg" ]; then
    for f in "$arg"/*.dylib "$arg"/*.so "$arg"/*.so.*; do
      [ -f "$f" ] && [ ! -L "$f" ] && files+=("$f")
    done
  else
    files+=("$arg")
  fi
done
if [ ${#files[@]} -eq 0 ]; then
  echo "FAIL: nothing to check in: $*" >&2
  exit 1
fi
# Only linked results record where each symbol comes from. Uses the platform's own
# object tools (already needed below), not file(1), which a minimal host may lack.
is_linked() {
  case "$(uname -s)" in
    Darwin) [ "$(xcrun otool -hv "$1" 2>/dev/null | grep -cE ' (DYLIB|EXECUTE|BUNDLE) ' || true)" -gt 0 ] ;;
    *) [ "$(readelf -h "$1" 2>/dev/null | grep -cE 'Type:[[:space:]]+(DYN|EXEC)' || true)" -gt 0 ] ;;
  esac
}
for f in "${files[@]}"; do
  if ! is_linked "$f"; then
    echo "FAIL: $f is not a shared library or executable; check the linked result instead" >&2
    exit 1
  fi
done

case "$(uname -s)" in
  Darwin)
    tbd="$(xcrun --show-sdk-path)/usr/lib/system/libsystem_m.tbd"
    system_math=$(grep -oE "_[A-Za-z0-9_]+" "$tbd" | sed 's/^_//' | sort -u)
    # Apple's nm -m (conda's nm doesn't print "(from libSystem)"). Weak imports read
    # "(undefined) weak external", so the symbol is taken by position, not by column.
    imports_of() {
      xcrun nm -m "$1" | sed -nE 's/^ *\(undefined\) (weak )?external _([^ ]+) \(from libSystem\).*/\2/p' | sort -u
    }
    # nm -m marks every symbol "external" or "non-external"; output without that marker
    # means nm didn't understand the file. (grep -c reads all of nm's output; grep -q
    # would stop early, and under pipefail the cut-off nm would count as a failure.)
    for f in "${files[@]}"; do
      [ "$(xcrun nm -m "$f" | grep -c "external" || true)" -gt 0 ] || { echo "FAIL: nm can't read $f" >&2; exit 1; }
    done
    ;;
  Linux)
    libm=$(ls /lib/*-linux-gnu/libm.so.6 /usr/lib/*-linux-gnu/libm.so.6 /lib64/libm.so.6 2>/dev/null | head -1 || true)
    [ -n "$libm" ] || { echo "FAIL: cannot find libm.so.6" >&2; exit 1; }
    system_math=$(nm -D --defined-only "$libm" | awk '{print $3}' | sed 's/@.*//' | sort -u)
    # Lists weak undefined symbols too (type "w").
    imports_of() { nm -D --undefined-only "$1" | awk '{print $NF}' | sed 's/@.*//' | sort -u; }
    # Every shared library or executable has dynamic symbols.
    for f in "${files[@]}"; do
      [ -n "$(nm -D "$f")" ] || { echo "FAIL: nm can't read $f" >&2; exit 1; }
    done
    ;;
esac

# The system math library must have been read, or nothing could ever be flagged.
if [ -z "$system_math" ]; then
  echo "FAIL: could not read the system math library's symbols" >&2
  exit 1
fi

bad=""
for f in "${files[@]}"; do
  math=$(comm -12 <(imports_of "$f") <(echo "$system_math"))
  inexact=$(echo "$math" | grep -vE "$exact" | grep -vE "$trap_enable" || true)
  traps=$(echo "$math" | grep -E "$trap_enable" || true)
  case "$(basename "$f")" in libTKernel.*) traps="" ;; esac
  for s in $inexact $traps; do bad="$bad\n  $(basename "$f"): $s"; done
done
if [ -n "$bad" ]; then
  printf "FAIL: imports of inexact platform math, or trap enabling outside TKernel:%b\n" "$bad" >&2
  exit 1
fi
echo "OK: only exact platform math functions imported (${#files[@]} files checked)"
