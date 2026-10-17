# PlaneGCS

The sketch solver, linked in the sketch worker (`crates/silico-sketch`), never ported (CLAUDE.md). It's FreeCAD's PlaneGCS as FreeCAD 1.1.3 ships it, the release the oracle runs ([VERSION](VERSION)), so Silico solves as the oracle does. FreeCAD's main has changed what the solver does since (docs/ports/sketcher.md); Silico's PlaneGCS moves only when its reference FreeCAD's does.

```bash
bash third_party/planegcs/scripts/verify.sh   # src/ is 1.1.3's PlaneGCS plus the series (pixi run test runs it)
```

- [src/](src/): the files of FreeCAD 1.1.3's release archive PlaneGCS needs, at their paths in it: `src/Mod/Sketcher/App/planegcs/` (five `.cpp` files and their headers), `src/Mod/Sketcher/SketcherGlobal.h`, and `src/boost_graph_adjacency_list.hpp`. LGPL-2.1-or-later ([LICENSE.FreeCAD](LICENSE.FreeCAD)): © 2011 Konstantinos Poulios; `Geo.cpp` © 2014 Victor Titov. The patches in [series](series) are applied in place.
- [pristine.sha256](pristine.sha256): every file of `src/` as the archive has it. [scripts/verify.sh](scripts/verify.sh) reverses the series on a copy and compares, so nothing changes in `src/` unrecorded.
- [stubs/](stubs/): stand-ins for the FreeCAD headers PlaneGCS includes (`FCGlobal.h`, whose own pulls in Qt; `Base/Console.h`, whose log lines are dropped; `FCConfig.h`, unused), and `infinity-nan.h`, force-included, since in C++20 the macOS SDK's `math.h` leaves `INFINITY` and `NAN` to a `<float.h>` the pinned libc++ doesn't pass on.
- Eigen 3.4.0 and Boost 1.86's headers come from pixi (`pixi.toml`), the versions the oracle's FreeCAD bundles.

`crates/silico-sketch/build.rs` compiles `src/` with the deterministic recipe (`target/occt/recipe.env`), `NDEBUG` as FreeCAD's release builds have it, `EIGEN_DONT_VECTORIZE`, and Eigen's register count fixed; the bridge fixes Eigen's cache sizes before the first solve and allocates every value of a system from one array in the core's order, since PlaneGCS orders its unknowns by address (docs/ports/sketcher.md, Determinism).

## Patches

Under the OCCT patch policy (`third_party/occt/README.md`), and single-threading allowed: whether to extend that policy to PlaneGCS is the maintainer's decision (docs/roadmap/m3-sketcher.md, open decision 7).

| Patch | Upstream | What it changes |
|---|---|---|
| [0001](patches/0001-dependent-parameters-on-the-calling-thread.patch) | None yet: the maintainer files it | `System::diagnose()` ran its dependent-parameter QR on a second thread (`std::async`'s default policy), joined before its result is read. It now runs deferred, on the calling thread, with the same arguments and result, so workers stay single-threaded (ARCHITECTURE.md, Deterministic kernel, 5). |

## Updating

When the oracle's FreeCAD moves: extract the same files from its release archive into `src/` (checking the archive against its published SHA-256, recorded in `VERSION`), write `pristine.sha256` again (`find src -type f | LC_ALL=C sort | xargs shasum -a 256`), re-apply the series with `patch -p1` (not `git apply`, which skips patches inside another repository), drop what upstream took, and rewrite the sketch stress reference only after reviewing why it moved (`SILICO_WRITE_SKETCH_STRESS=1`, `crates/silico-sketch/tests/stress.rs`).
