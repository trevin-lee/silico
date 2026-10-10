#!/usr/bin/env bash
# Proves every build guard rejects what it exists to reject, so an edit can't turn
# one into a check that always passes. Needs a completed `pixi run occt`. (The shared
# build cache has its own tests, in test-cache.sh, which need no build.)
# Run through pixi:  pixi run test-guards
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
scripts=$root/third_party/occt/scripts
out=$root/target/occt
# shellcheck disable=SC1091
. "$out/recipe.env"
# shellcheck disable=SC1091
. "$scripts/platform.sh"
# shellcheck disable=SC1091
. "$scripts/test-lib.sh"

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
  guard_broken "fold-canary didn't check all $listed calls from inexact-math.txt"
fi

# Math imports: a library calling the platform's sin fails.
cat > "$tmp/uses_sin.cpp" <<'EOF'
#include <cmath>
extern "C" double uses_sin(double x) { return std::sin(x) + std::pow(x, 1.5); }
EOF
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" -o "$tmp/libTKFake.$lib_ext"
expect_fail "check-math-imports" bash "$scripts/check-math-imports.sh" "$tmp/libTKFake.$lib_ext"
expect_fail "check-math-imports (empty input)" bash "$scripts/check-math-imports.sh" "$tmp/pristine/adm"
# A weak import of sin.
cat > "$tmp/weak_sin.cpp" <<'CPP'
extern "C" double sin(double) __attribute__((weak));
extern "C" double uses_weak_sin(double x) { return sin(x); }
CPP
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/weak_sin.cpp" -o "$tmp/libTKWeak.$lib_ext"
expect_fail "check-math-imports (weak import)" bash "$scripts/check-math-imports.sh" "$tmp/libTKWeak.$lib_ext"
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
  clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/traps.cpp" -o "$tmp/libTKTraps.$lib_ext"
  expect_fail "check-math-imports (traps outside TKernel)" bash "$scripts/check-math-imports.sh" "$tmp/libTKTraps.$lib_ext"
fi
expect_pass "check-math-imports" bash "$scripts/check-math-imports.sh" "$out/install/lib"

# TKernel may enable traps, but its other math is checked like any library's.
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" -o "$tmp/libTKernel.$lib_ext"
expect_fail_with "check-math-imports (TKernel importing sin)" "libTKernel.$lib_ext: sin" \
  bash "$scripts/check-math-imports.sh" "$tmp/libTKernel.$lib_ext"
if [ "$(uname -s)" = Linux ]; then
  mkdir -p "$tmp/tkernel"
  # shellcheck disable=SC2086
  clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/traps.cpp" -o "$tmp/tkernel/libTKernel.$lib_ext"
  expect_pass "check-math-imports (TKernel enabling traps)" bash "$scripts/check-math-imports.sh" "$tmp/tkernel/libTKernel.$lib_ext"
fi

# Exceptions: an OCCT whose input checks are compiled out (one file rebuilt with
# -DNo_Exception, found ahead of the real library) must fail, for the right reason.
mkdir -p "$tmp/noexc"
# shellcheck disable=SC2046,SC2086
"$SILICO_CXX" -shared -fPIC -std=c++17 $SILICO_CXXFLAGS $SILICO_LDFLAGS -DNo_Exception $(soname_flag "libTKMathNoExc.$lib_ext") \
  -I"$out/install/include/opencascade" "$out/src/src/gp/gp_Trsf.cxx" \
  -o "$tmp/noexc/libTKMathNoExc.$lib_ext" -L"$out/install/lib" -lTKMath -lTKernel -Wl,-rpath,"$out/install/lib"
ln -s "libTKMathNoExc.$lib_ext" "$tmp/noexc/libTKMath.$lib_ext"
expect_fail_with "check-exceptions" "accepted a zero scale" bash "$scripts/check-exceptions.sh" "$tmp/noexc"
expect_pass "check-exceptions" bash "$scripts/check-exceptions.sh"

# Shim privacy: a library that links the shim without hiding it fails.
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS "$tmp/uses_sin.cpp" "$SILICO_LIBM_SHIM" -o "$tmp/libTKLeaky.$lib_ext"
expect_fail "check-shim-hidden" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM" "$tmp/libTKLeaky.$lib_ext"
expect_fail "check-shim-hidden (no libraries)" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM"
# shellcheck disable=SC2086
clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS $SILICO_SHIM_HIDE_FLAG "$tmp/uses_sin.cpp" "$SILICO_LIBM_SHIM" -o "$tmp/libTKPrivate.$lib_ext"
expect_pass "check-shim-hidden" bash "$scripts/check-shim-hidden.sh" "$SILICO_LIBM_SHIM" "$tmp/libTKPrivate.$lib_ext"
expect_pass "check-math-imports (shim linked)" bash "$scripts/check-math-imports.sh" "$tmp/libTKPrivate.$lib_ext"

# Relocatability. Each fixture is an install whose first library is good and whose last
# breaks one rule (the real install has dozens of libraries, so a check that stops early
# must fail). Fixture libraries are named by @rpath and search only their own directory,
# so the rule under test is the only thing wrong.
reloc_lib() { # <library> <source>
  # shellcheck disable=SC2046,SC2086
  clang++ -shared -fPIC $SILICO_CXXFLAGS $SILICO_LDFLAGS $(soname_flag "$(basename "$1")") "$2" -o "$1"
  set_rpaths "$1" "$own_dir_rpath"
}
reloc_install() { # <name>: prints the new install's directory
  mkdir -p "$tmp/$1/lib"
  reloc_lib "$tmp/$1/lib/libTKAGood.$lib_ext" "$tmp/uses_sin.cpp"
  echo "$tmp/$1"
}
printf 'extern "C" const char* where() { return "%s/project/file.cxx"; }\n' "$tmp" > "$tmp/where.cpp"
reloc=$(reloc_install reloc-good)
expect_pass "check-relocatable (fixture)" bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
reloc=$(reloc_install reloc-fixed)
reloc_lib "$reloc/lib/libTKZFixed.$lib_ext" "$tmp/uses_sin.cpp"
set_rpaths "$reloc/lib/libTKZFixed.$lib_ext" "$own_dir_rpath" /opt/somewhere
expect_fail_with "check-relocatable (fixed search path)" "libTKZFixed.$lib_ext searches /opt/somewhere" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
reloc=$(reloc_install reloc-up)
reloc_lib "$reloc/lib/libTKZUp.$lib_ext" "$tmp/uses_sin.cpp"
set_rpaths "$reloc/lib/libTKZUp.$lib_ext" "$own_dir_rpath/../lib"
expect_fail_with "check-relocatable (search path outside its directory)" "libTKZUp.$lib_ext searches $own_dir_rpath/../lib" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
reloc=$(reloc_install reloc-string)
reloc_lib "$reloc/lib/libTKZString.$lib_ext" "$tmp/where.cpp"
expect_fail_with "check-relocatable (path in a library's strings)" "libTKZString.$lib_ext contains a path under" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
reloc=$(reloc_install reloc-script)
mkdir -p "$reloc/bin"
echo "export THIRDPARTY_DIR=$tmp/src" > "$reloc/bin/env.sh"
expect_fail_with "check-relocatable (path in another file)" "bin/env.sh contains a path under" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
reloc=$(reloc_install reloc-junk)
echo "not a library" > "$reloc/lib/libTKZJunk.$lib_ext"
expect_fail_with "check-relocatable (unreadable library)" "libTKZJunk.$lib_ext can't be read" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
# A prefix is forbidden in its physical form too, and the failure names the form that
# matched: here the library holds the real path, and the prefix given is a symlink to it.
mkdir -p "$tmp/real"
ln -s "$tmp/real" "$tmp/alias"
real=$(cd "$tmp/real" && pwd -P)
printf 'extern "C" const char* where() { return "%s/file.cxx"; }\n' "$real" > "$tmp/where-real.cpp"
reloc=$(reloc_install reloc-physical)
reloc_lib "$reloc/lib/libTKZPhysical.$lib_ext" "$tmp/where-real.cpp"
expect_fail_with "check-relocatable (path through a symlinked prefix)" "libTKZPhysical.$lib_ext contains a path under $real\$" \
  bash "$scripts/check-relocatable.sh" "$reloc" "$tmp/alias"
# A two-component prefix, like a home directory, is still forbidden. (It needn't exist.)
reloc=$(reloc_install reloc-home)
mkdir -p "$reloc/share"
echo "/silico-fake-home/me/occt/file.cxx" > "$reloc/share/where.txt"
expect_fail_with "check-relocatable (two-component prefix)" "share/where.txt contains a path under /silico-fake-home/me" \
  bash "$scripts/check-relocatable.sh" "$reloc" /silico-fake-home/me
# grep can't read a file: fail, don't pass. (root reads anything, so it's skipped there.)
if [ "$(id -u)" != 0 ]; then
  reloc=$(reloc_install reloc-locked)
  mkdir -p "$reloc/share"
  echo "$tmp/secret" > "$reloc/share/locked.txt"
  chmod 000 "$reloc/share/locked.txt"
  expect_fail_with "check-relocatable (unreadable file)" "couldn't read" \
    bash "$scripts/check-relocatable.sh" "$reloc" "$tmp"
else
  echo "skip: check-relocatable (unreadable file): running as root, which reads anything"
fi
mkdir -p "$tmp/reloc-empty/lib"
expect_fail_with "check-relocatable (no libraries)" "no libraries" \
  bash "$scripts/check-relocatable.sh" "$tmp/reloc-empty" "$tmp"
# One-component prefixes can't identify a machine and match OCCT's own text (occt/src/...,
# its default /tmp), so they're skipped; with nothing else to check, the check fails.
expect_fail_with "check-relocatable (no prefixes)" "no forbidden prefixes" \
  bash "$scripts/check-relocatable.sh" "$out/install"
expect_fail_with "check-relocatable (only one-component prefixes)" "no forbidden prefixes" \
  bash "$scripts/check-relocatable.sh" "$out/install" / /silico-no-such-dir
expect_pass "check-relocatable (checkout at /src, home at /tmp)" \
  bash "$scripts/check-relocatable.sh" "$out/install" /src /src/ /tmp /app "$tmp"
expect_pass "check-relocatable" \
  bash "$scripts/check-relocatable.sh" "$out/install" "$(cd -P "$out/install/../.." && pwd)" "$root" "$HOME"

finish_guards test-guards
