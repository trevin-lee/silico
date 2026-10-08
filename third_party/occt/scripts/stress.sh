#!/usr/bin/env bash
# stress_corpus_bit_identical: runs the 605-case stress test 10 times against the OCCT
# built by build.sh and requires every run to match the committed reference byte for byte.
#
# Run through pixi:  pixi run stress
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
occt=$root/target/occt/install
dir=$root/tests/determinism
# One reference for every platform: the results are bit-identical on macOS and Linux.
reference=$dir/stress-reference.json
work=$root/target/occt/stress
runs=${STRESS_RUNS:-10}
case "$runs" in
  '' | *[!0-9]*) runs_ok=false ;;
  *) [ "$((10#$runs))" -ge 1 ] && runs_ok=true || runs_ok=false ;;
esac
if [ "$runs_ok" != true ]; then
  echo "stress: STRESS_RUNS must be a positive integer, got '$runs'" >&2
  exit 1
fi
runs=$((10#$runs))
mkdir -p "$work"

# Build with the same recipe as OCCT.
# shellcheck disable=SC1091
. "$root/target/occt/recipe.env"

libs=(TKernel TKMath TKG2d TKG3d TKGeomBase TKGeomAlgo TKBRep TKTopAlgo TKPrim TKBO TKBool TKFillet TKOffset TKMesh TKShHealing)
clang++ -std=c++17 -O2 $SILICO_CXXFLAGS $SILICO_LDFLAGS \
  -I"$occt/include/opencascade" "$dir/stress.cpp" -o "$work/stress" \
  -L"$occt/lib" "${libs[@]/#/-l}" \
  -Wl,-rpath,"$occt/lib" -Wl,-rpath,"$CONDA_PREFIX/lib" 2> "$work/compile.log" \
  || { cat "$work/compile.log" >&2; exit 1; }

# The harness itself must not use platform math either.
bash "$root/third_party/occt/scripts/check-math-imports.sh" "$work/stress"

failed=0
for i in $(seq 1 "$runs"); do
  "$work/stress" "$work/run$i.json"
  if ! cmp -s "$reference" "$work/run$i.json"; then
    echo "stress: run $i differs from the reference:" >&2
    python3 -I "$dir/compare-stress.py" "$reference" "$work/run$i.json" | sed -n '/TOTAL/,$p' | head -12 >&2
    failed=1
  fi
done
if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "stress: $runs of $runs runs match the reference byte for byte (605 cases)"
