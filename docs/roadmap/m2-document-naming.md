# M2 – Document and naming

**Status: outline.** Detail these steps once the two study notes below are written. The study shapes the steps, especially M2.7 and M2.8.

**Goal:** the JSON timeline becomes a working parametric model. It parses, validates, recomputes through the worker pool, caches per feature, checks fingerprints, and names topology stably.

**Exit tests:** golden models rebuild byte-for-byte deterministically, the ported FreeCAD naming tests pass, references survive upstream edits or fail loudly, and the council approves with `/adversarial-review --milestone`.

## Study first

- **`docs/ports/naming.md`**, from:
  - `FreeCAD/src/Mod/Part/App/TopoShapeExpansion.cpp` (6,455 lines);
  - `FreeCAD/src/App/ElementMap.cpp`, `MappedName.*`, `StringHasher.*`;
  - the 92 tests in `FreeCAD/tests/src/Mod/Part/App/TopoShapeExpansion.cpp`;
  - `FreeCAD/src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py`;
  - the git history of those files, especially crash and regression fixes.

  The note decides Silico's readable name syntax, which is an open question in ARCHITECTURE.md.
- **`docs/ports/recompute.md`**, from the recompute and dependency logic in `FreeCAD/src/App/Document.cpp`, with tests in `FreeCAD/tests/src/App/Document.cpp`.

## Steps

| Step | Builds | Key tests |
|---|---|---|
| M2.1 Schema and canonical JSON | Rust types for the document, the canonical writer and parser, and a JSON Schema generated with `schemars` | Byte-stable round trip; float formatting (shortest form, no `-0`); unknown keys rejected; a fuzzed parser never panics; the schema snapshot is stable |
| M2.2 Validation gate | Unique IDs, existing references, cycle detection, feature versions, units | One structured error per kind of invalid document, each naming the feature and field |
| M2.3 Kernel trait and fake kernel | A `Kernel` trait that the worker pool implements, plus an in-memory fake | Core tests run without OCCT; the fake records calls, so tests can assert what was recomputed |
| M2.4 DAG recompute | Topological order, minimal invalidation, parallel independent branches, cancellation, generations | An edit recomputes only the downstream features; independent branches overlap in time; a cancelled recompute leaves no partial state |
| M2.5 Cache | `.silico-cache/`, keyed by the feature, its inputs, and the kernel version | A hit returns identical geometry; changing any input misses; a kernel version bump misses; a corrupt cache entry is rebuilt, not trusted |
| M2.6 Fingerprints | Compute, store, compare, report drift | A tampered fingerprint is reported as drift that names the feature; the fingerprint survives a round trip |
| M2.7 Element map | Port of naming from history (M1.4) | Ported FreeCAD `TopoShapeExpansion` tests, with expected names translated to Silico's syntax |
| M2.8 Reference resolution | References resolved by name after an upstream edit | Ported `TestTopologicalNamingProblem.py` cases; every edit either resolves to the geometrically equivalent element or fails loudly, never silently re-points |
| M2.9 CLI | `silico check`, `fmt`, `show`, and a git `textconv` driver | Each command is snapshot-tested on golden models; `fmt` is idempotent; the `git diff` output of a one-parameter edit is one readable line |
