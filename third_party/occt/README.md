# OCCT

Silico builds OpenCascade Technology from source: the release pinned in [VERSION](VERSION), with the patches in [series](series) applied in order, using the deterministic recipe in ARCHITECTURE.md's *Deterministic kernel*.

```bash
pixi run occt          # build (cached), then run every check below
pixi run test-guards   # prove each check rejects bad input
pixi run stress        # 605-case stress test, byte-identical to the reference
```

- [build.sh](build.sh): fetch, verify, patch, build, check, and write `target/occt/recipe.env` for other worker C++. Builds are shared between checkouts through a cache keyed by the build's inputs (`~/.cache/silico/occt`, or under `$XDG_CACHE_HOME` or `$SILICO_OCCT_CACHE`); delete it to start fresh. [scripts/cache.sh](scripts/cache.sh) describes how entries are kept.
- [libm-shim/](libm-shim/): the platform math library replacement, backed by Rust's `libm`.
- [scripts/](scripts/): `inexact-math.txt` (the function list behind the no-folding flags and the canary), `fold-canary.sh`, `verify-patches.sh`, `check-math-imports.sh`, `check-shim-hidden.sh`, `check-exceptions.sh`, `check-relocatable.sh`, `cache.sh` (publishing, using, and pruning shared builds, under one lock), `older-layouts.sh`, `platform.sh` (commands that differ between macOS and Linux, shared by several scripts), `test-guards.sh` and `test-cache.sh` (with shared helpers in `test-lib.sh`), and `stress.sh`.

## Patch policy

- A patch must fix determinism or a crash that Silico can't work around, or expose what OCCT already computes as accessors that change no result (`pixi run stress` stays byte-identical). No other features.
- A patch may also fix a wrong result of OCCT's precise measurement (`BRepGProp` with an `Eps`) that Silico can't work around exactly on its own side, if it leaves the default integration (FreeCAD's: `measure_as_freecad`, the oracle, the stress reference) byte-identical, its expected values are by hand and independent of OCCT, and it names every committed fingerprint and protected reference it moves.
- Every patch must exist upstream as a pull request or issue, and its header says where.
- Each patch header states why Silico carries it and when to drop it.
- When OCCT is upgraded, drop patches the new release includes, re-apply the rest, and run `pixi run test` and `pixi run stress` on macOS and Linux.

## Patches

| Patch | Upstream | What it fixes |
|---|---|---|
| [0001](patches/0001-offset-deterministic-face-order.patch) | [PR #1600](https://github.com/Open-Cascade-SAS/OCCT/pull/1600), backported to 7.9.3 | Arc-join offsets returned their faces in an order that depended on memory addresses, so results changed from run to run. |
| [0002](patches/0002-chamfer-faulty-report.patch) | [issue #1632](https://github.com/Open-Cascade-SAS/OCCT/issues/1632) | `BRepFilletAPI_MakeChamfer` didn't pass on its builder's report of where a failed build failed, as `BRepFilletAPI_MakeFillet` does. Accessors only. |
| [0003](patches/0003-gprop-revolution-u-spans.patch) | ISSUE-TO-COME | The precise measurement (`BRepGProp` with an `Eps`) integrated a surface of revolution's angle as one span where it splits a torus's into three, so a full-turn barrel measured 2.2e-7 low and a revolved multi-span B-spline 3.9e-6 low. The default integration is unchanged. |

## Applying the patches by hand (inspection only)

```bash
tar -xzf V7_9_3.tar.gz && cd OCCT-7_9_3
while read -r p; do patch -p1 --forward --batch < "../third_party/occt/patches/$p"; done < ../third_party/occt/series
```

This doesn't give a deterministic build: use `pixi run occt` for that. Use `patch`, not `git apply`: inside another git repository (such as Silico's), `git apply` resolves paths against that repository's root, prints "Skipped patch", and exits 0 without changing anything. The build script must check that every patch applied.
