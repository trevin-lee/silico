# M2 – Document and naming

**Status: outline.** Detail these steps once the two study notes below are written. The study shapes the steps, especially M2.7 and M2.8.

**Goal:** the JSON timeline becomes a working parametric model. It parses, validates, recomputes through the worker pool, caches per feature, checks fingerprints, and names topology stably.

**Exit tests:** golden models save and rebuild byte-identically on every platform, the ported FreeCAD naming tests pass, references survive upstream edits or fail loudly, and the council approves with `/adversarial-review --milestone`.

## Study first

- **`docs/ports/naming.md`**, from:
  - `~/Git/FreeCAD/src/Mod/Part/App/TopoShapeExpansion.cpp` (6,455 lines);
  - `~/Git/FreeCAD/src/App/ElementMap.cpp`, `MappedName.*`, `StringHasher.*`;
  - the 92 tests in `~/Git/FreeCAD/tests/src/Mod/Part/App/TopoShapeExpansion.cpp`;
  - `~/Git/FreeCAD/src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py`;
  - the git history of those files, especially crash and regression fixes.

  The note decides Silico's readable name syntax, which is an open question in ARCHITECTURE.md.
- **`docs/ports/recompute.md`**, from the recompute and dependency logic in `~/Git/FreeCAD/src/App/Document.cpp`, with tests in `~/Git/FreeCAD/tests/src/App/Document.cpp`.

## Steps

| Step | Builds | Key tests |
|---|---|---|
| M2.0 Refusals as data (first, agreed at the M1 review) | The worker sends what it refused as data: the elements at fault as `ElementRef`s (every one, not the first) and a reason, not text. The core names the elements, writes the message and the suggestion, and fills `candidates` (finding a multi-edge fillet's failing edge by trying the edges across the pool). An invalid parameter is named as data. A mesh's unmeshed faces come back as data too, so the core decides whether to leave them out or refuse (the worker decides today, naming the face). This changes the protocol and the error contract (`tests/contract/`), as the maintainer agreed | A refusal of edges 3 and 5 carries both as elements, through the core; the worker writes no element names; every refusal's message and suggestion come from the core; the contract's JSON shapes are updated with the maintainer |
| M2.1 Schema and canonical JSON | Rust types for the document, including fixed-point `Length` and `Angle` and the `units` header; the canonical writer and parser; a JSON Schema generated with `schemars` | Byte-stable round trip; `0.1` parses to exactly 100,000 nm without touching a float; too many decimals, out-of-range values, `NaN`, and `-0` are rejected with structured errors; units other than `mm`/`1nm` are rejected in version 1; unknown keys rejected; a fuzzed parser never panics; the schema snapshot is stable |
| M2.2 Validation gate | Unique IDs, existing references, cycle detection, feature versions, units | One structured error per kind of invalid document, each naming the feature and field |
| M2.3 Kernel trait and fake kernel | A `Kernel` trait that the worker pool implements, plus an in-memory fake | Core tests run without OCCT; the fake records calls, so tests can assert what was recomputed |
| M2.4 DAG recompute | Topological order, minimal invalidation, parallel independent branches, cancellation, generations | An edit recomputes only the downstream features; independent branches overlap in time; a cancelled recompute leaves no partial state |
| M2.5 Cache | `.silico-cache/`, keyed by the feature, its inputs, the kernel version, and the platform; each entry is the B-rep and the worker's word on its validity, under one checksum, so a hit is a `ShapeData::checked` | A hit returns identical geometry; changing any input misses; a kernel version bump misses; a different platform misses; a corrupt cache entry is rebuilt, not trusted |
| M2.6 Fingerprints | Compute, quantize, store, compare, report drift | A tampered fingerprint fails `silico check` as drift that names the feature; a rebuild of an unchanged document leaves the file's bytes unchanged; an edit rewrites exactly the fingerprints of the features it affects; the fingerprint survives a round trip |
| M2.7 Element map | Port of naming from history (M1.4). A placement or move keeps every element's name, as FreeCAD's `makeElementTransform` does: the kernel's translate reports its elements modified, and those fates must not feed the element map (docs/ports/booleans.md) | Ported FreeCAD `TopoShapeExpansion` tests, with expected names translated to Silico's syntax |
| M2.8 Reference resolution | References resolved by name after an upstream edit | Ported `TestTopologicalNamingProblem.py` cases; every edit either resolves to the geometrically equivalent element or fails loudly, never silently re-points |
| M2.9 CLI | `silico check`, `fmt`, `show`, and a git `textconv` driver | Each command is snapshot-tested on golden models; `fmt` is idempotent; the `git diff` output of a one-parameter edit is one readable line |

## As built

### M2.0 – Refusals as data

M2.0 is built in parts, each reviewed on its own:
- **The error contract** (done). The maintainer chose the shape: `elements`, a list of every sub-element at fault, replaces `element`; `parameter` names the parameter at fault as a dotted name: the operation's field, and the part of it at fault where it has parts (`radius`, `offset.x`, a chamfer's `size.angle`, an extrusion's `profile`); and `resource_exhausted`, appended to the codes, is a shortage of files, processes, memory, or buffers, which `internal` (a bug in Silico) had reported. Every value the request constructors check is refused naming its parameter (a request too large to send names none). The worker still sends one element, by name.
- **Refusals from the worker as data**: a reason and `ElementRef`s, every element at fault; the core writes the names, the message, and the suggestion.
- **Unmeshed faces as data**, the core deciding whether to leave them out or refuse.
- **A multi-edge fillet's failing edges** found by the core, trying the edges across the pool, as `candidates`.
