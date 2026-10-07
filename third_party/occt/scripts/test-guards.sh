#!/usr/bin/env bash
# Proves every build guard rejects what it exists to reject, so an edit can't turn
# one into a check that always passes. Needs a completed `pixi run occt`.
# Run through pixi:  pixi run test-guards
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
scripts=$root/third_party/occt/scripts
out=$root/target/occt
# shellcheck disable=SC1091
. "$out/install/share/silico/recipe.env"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
case "$(uname -s)" in
  Darwin) ext=dylib ;;
  *) ext=so ;;
esac

failures=0
expect_fail() {
  local name=$1
  shift
  if "$@" >"$tmp/out.log" 2>&1; then
    echo "GUARD BROKEN: $name passed on bad input" >&2
    failures=$((failures + 1))
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
    echo "GUARD BROKEN: $name passed on bad input" >&2
    failures=$((failures + 1))
  elif grep -q "$pattern" "$tmp/out.log"; then
    echo "ok: $name rejects bad input"
  else
    echo "GUARD BROKEN: $name failed, but not with \"$pattern\":" >&2
    cat "$tmp/out.log" >&2
    failures=$((failures + 1))
  fi
}
expect_pass() {
  local name=$1
  shift
  if "$@" >"$tmp/out.log" 2>&1; then
    echo "ok: $name accepts good input"
  else
    echo "GUARD BROKEN: $name failed on good input:" >&2
    cat "$tmp/out.log" >&2
    failures=$((failures + 1))
  fi
}

# Patches: unpatched source fails, the built source passes.
tag=$(sed -n 's/^tag=//p' "$root/third_party/occt/VERSION")
mkdir -p "$tmp/pristine"
tar -xzf "$out/downloads/OCCT-$tag.tar.gz" -C "$tmp/pristine" --strip-components=1
expect_fail "verify-patches" bash "$scripts/verify-patches.sh" "$tmp/pristine"
expect_pass "verify-patches" bash "$scripts/verify-patches.sh" "$out/src"

# Folding: plain flags fold math at build time, the recipe doesn't. The passing run must
# check every function in inexact-math.txt (double and float), not some subset.
expect_fail "fold-canary" bash "$scripts/fold-canary.sh" -stdlib=libc++
expect_fail "fold-canary (-fmath-errno alone)" bash "$scripts/fold-canary.sh" -fmath-errno -stdlib=libc++
# shellcheck disable=SC2086
expect_pass "fold-canary" bash "$scripts/fold-canary.sh" $SILICO_CXXFLAGS
listed=$(awk '!/^#/ && NF && $3 != "nocanary" {n++} END {print n * 2}' "$scripts/inexact-math.txt")
if ! grep -q "($listed checked)" "$tmp/out.log"; then
  echo "GUARD BROKEN: fold-canary didn't check all $listed calls from inexact-math.txt" >&2
  failures=$((failures + 1))
fi

# Math imports: a library calling the platform's sin fails.
cat > "$tmp/uses_sin.cpp" <<'EOF'
#include <cmath>
extern "C" double uses_sin(double x) { return std::sin(x) + std::pow(x, 1.5); }
EOF
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" -o "$tmp/libTKFake.$ext"
expect_fail "check-math-imports" bash "$scripts/check-math-imports.sh" "$tmp/libTKFake.$ext"
expect_fail "check-math-imports (empty input)" bash "$scripts/check-math-imports.sh" "$tmp/pristine/adm"
# A weak import of sin.
cat > "$tmp/weak_sin.cpp" <<'CPP'
extern "C" double sin(double) __attribute__((weak));
extern "C" double uses_weak_sin(double x) { return sin(x); }
CPP
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/weak_sin.cpp" -o "$tmp/libTKWeak.$ext"
expect_fail "check-math-imports (weak import)" bash "$scripts/check-math-imports.sh" "$tmp/libTKWeak.$ext"
# A static archive or object file can't be checked: it must be rejected, not passed.
# shellcheck disable=SC2086
clang++ -c -fPIC $SILICO_CXXFLAGS "$tmp/uses_sin.cpp" -o "$tmp/uses_sin.o"
ar rcs "$tmp/libbridge.a" "$tmp/uses_sin.o"
expect_fail "check-math-imports (static archive)" bash "$scripts/check-math-imports.sh" "$tmp/libbridge.a"
expect_fail "check-math-imports (object file)" bash "$scripts/check-math-imports.sh" "$tmp/uses_sin.o"
# Enabling FP traps outside TKernel (glibc only: macOS has no feenableexcept).
if [ "$(uname -s)" = Linux ]; then
  cat > "$tmp/traps.cpp" <<'CPP'
#include <fenv.h>
extern "C" int enable_traps() { return feenableexcept(FE_DIVBYZERO); }
CPP
  # shellcheck disable=SC2086
  clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/traps.cpp" -o "$tmp/libTKTraps.$ext"
  expect_fail "check-math-imports (traps outside TKernel)" bash "$scripts/check-math-imports.sh" "$tmp/libTKTraps.$ext"
fi
expect_pass "check-math-imports" bash "$scripts/check-math-imports.sh" "$out/install/lib"

# TKernel may enable traps, but its other math is checked like any library's.
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" -o "$tmp/libTKernel.$ext"
expect_fail_with "check-math-imports (TKernel importing sin)" "libTKernel.$ext: sin" \
  bash "$scripts/check-math-imports.sh" "$tmp/libTKernel.$ext"
if [ "$(uname -s)" = Linux ]; then
  mkdir -p "$tmp/tkernel"
  # shellcheck disable=SC2086
  clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/traps.cpp" -o "$tmp/tkernel/libTKernel.$ext"
  expect_pass "check-math-imports (TKernel enabling traps)" bash "$scripts/check-math-imports.sh" "$tmp/tkernel/libTKernel.$ext"
fi

# Exceptions: an OCCT whose input checks are compiled out (one file rebuilt with
# -DNo_Exception, found ahead of the real library) must fail, for the right reason.
mkdir -p "$tmp/noexc"
case "$(uname -s)" in
  Darwin) soname="-Wl,-install_name,@rpath/libTKMathNoExc.$ext" ;;
  *) soname="-Wl,-soname,libTKMathNoExc.$ext" ;;
esac
# shellcheck disable=SC2086
"$SILICO_CXX" -shared -fPIC -std=c++17 $SILICO_CXXFLAGS $SILICO_LDFLAGS -DNo_Exception $soname \
  -I"$out/install/include/opencascade" "$out/src/src/FoundationClasses/TKMath/gp/gp_Trsf.cxx" \
  -o "$tmp/noexc/libTKMathNoExc.$ext" -L"$out/install/lib" -lTKMath -lTKernel -Wl,-rpath,"$out/install/lib"
ln -s "libTKMathNoExc.$ext" "$tmp/noexc/libTKMath.$ext"
expect_fail_with "check-exceptions" "accepted a zero scale" bash "$scripts/check-exceptions.sh" "$tmp/noexc"
expect_pass "check-exceptions" bash "$scripts/check-exceptions.sh"

# Shim privacy: a library that links the shim without hiding it fails.
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" "$SILICO_LIBM_SHIM" -o "$tmp/libTKLeaky.$ext"
expect_fail "check-shim-hidden" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM" "$tmp/libTKLeaky.$ext"
expect_fail "check-shim-hidden (no libraries)" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM"
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS $SILICO_SHIM_HIDE_FLAG "$tmp/uses_sin.cpp" "$SILICO_LIBM_SHIM" -o "$tmp/libTKPrivate.$ext"
expect_pass "check-shim-hidden" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM" "$tmp/libTKPrivate.$ext"
expect_pass "check-math-imports (shim linked)" bash "$scripts/check-math-imports.sh" "$tmp/libTKPrivate.$ext"

if [ "$failures" -ne 0 ]; then
  echo "test-guards: $failures guard(s) broken" >&2
  exit 1
fi
echo "test-guards: every guard rejects bad input and accepts good input"
