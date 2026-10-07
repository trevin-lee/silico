# OCCT

Silico builds OpenCascade Technology from source: the release pinned in [VERSION](VERSION), with the patches in [series](series) applied in order, using the deterministic recipe in ARCHITECTURE.md's *Deterministic kernel*.

```bash
pixi run occt          # build (cached), then run every check below
pixi run test-guards   # prove each check rejects bad input
pixi run stress        # 605-case stress test, byte-identical to the reference
```

- [build.sh](build.sh): fetch, verify, patch, build, check, and write `recipe.env` for other worker C++.
- [libm-shim/](libm-shim/): the platform math library replacement, backed by Rust's `libm`.
- [scripts/](scripts/): `inexact-math.txt` (the function list behind the no-folding flags and the canary), `fold-canary.sh`, `verify-patches.sh`, `check-math-imports.sh`, `check-shim-hidden.sh`, `check-exceptions.sh`, `test-guards.sh`, and `stress.sh`.

## Patch policy

- A patch must fix determinism or a crash that Silico can't work around. No features.
- Every patch must exist upstream as a pull request or issue, and its header says where.
- Each patch header states why Silico carries it and when to drop it.
- When OCCT is upgraded, drop patches the new release includes, re-apply the rest, and run `pixi run test` and `pixi run stress` on macOS and Linux.

## Patches

| Patch | Upstream | What it fixes |
|---|---|---|
| [0001](patches/0001-offset-deterministic-face-order.patch) | [PR #1600](https://github.com/Open-Cascade-SAS/OCCT/pull/1600) | Arc-join offsets returned their faces in an order that depended on memory addresses, so results changed from run to run. |

## Applying the patches by hand (inspection only)

```bash
tar -xzf V8_0_1.tar.gz && cd OCCT-8_0_1
while read -r p; do patch -p1 --forward --batch < "../third_party/occt/patches/$p"; done < ../third_party/occt/series
```

This doesn't give a deterministic build: use `pixi run occt` for that. Use `patch`, not `git apply`: inside another git repository (such as Silico's), `git apply` resolves paths against that repository's root, prints "Skipped patch", and exits 0 without changing anything. The build script must check that every patch applied.
