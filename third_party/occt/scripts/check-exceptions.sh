#!/usr/bin/env bash
# Fails unless OCCT throws on invalid input. OCCT's Release default compiles its input
# checks out (-DNo_Exception), turning errors into quiet nonsense.
# Usage: check-exceptions.sh [OCCT lib dir searched first]
# Reads the recipe from $SILICO_RECIPE (default: the installed recipe.env).
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
occt=$root/target/occt/install
# shellcheck disable=SC1090
. "${SILICO_RECIPE:-$occt/share/silico/recipe.env}"
first=${1:-$occt/lib}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/throws.cpp" <<'CPP'
#include <Standard_Failure.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <cstdio>
int main() {
  try {
    gp_Trsf t;
    t.SetScale(gp_Pnt(0, 0, 0), 0.0);  // invalid: a zero scale
  } catch (const Standard_Failure&) {
    return 0;
  }
  std::fprintf(stderr, "OCCT accepted a zero scale without throwing\n");
  return 1;
}
CPP
# shellcheck disable=SC2086
"$SILICO_CXX" -std=c++17 $SILICO_CXXFLAGS $SILICO_LDFLAGS -I"$occt/include/opencascade" \
  "$tmp/throws.cpp" "$SILICO_LIBM_SHIM" -o "$tmp/throws" \
  -L"$first" -L"$occt/lib" -lTKMath -lTKernel \
  -Wl,-rpath,"$first" -Wl,-rpath,"$occt/lib" -Wl,-rpath,"$(dirname "$(dirname "$SILICO_CXX")")/lib"
if ! "$tmp/throws"; then
  echo "FAIL: OCCT doesn't throw on invalid input (built with No_Exception?)" >&2
  exit 1
fi
echo "OK: OCCT throws Standard_Failure on invalid input"
