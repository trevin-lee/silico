# Recompute

How a document's features are rebuilt after an edit: in what order, which ones, on which workers, what happens when one fails, and what's left when a recompute is cancelled. Built in M2.4 on the `Kernel` trait (M2.3), for a document the gate has passed (M2.2). The cache (M2.5) and fingerprints (M2.6) build on the keys and results described here.

## What FreeCAD does

`Document::recompute` (`src/App/Document.cpp`, around line 2855) and `DocumentObject` (`src/App/DocumentObject.cpp`, lines 150 to 320). Baseline `876271d660`.

- **Order.** `getDependencyList(objs, DepSort)` walks each object's `OutList` (what it uses) and sorts the result with `boost::topological_sort`. The document's own order plays no part. `DocumentP::topologicalSort` (Kahn's algorithm) still exists, but recompute doesn't use it: a FIXME from realthunder says it can't handle partial recompute.
- **What recomputes.** State bits on each object, not a comparison of values. A property change sets `Touch`, and `Enforce` unless the property is `Prop_NoRecompute` (a placement: the object is touched but not re-executed). An object runs when `mustRecompute()`: `Enforce` is set, or `mustExecute()` (its expressions or extensions say so). After an object runs, each object in its `InList` (its users) gets `enforceRecompute()`, so the change ripples downstream one level at a time, in sorted order. `Freeze` excludes an object (`6c9ad95a70`).
- **Partial recompute.** `recompute([objs])` runs the given objects and what they use, never their users: `testSimple` leaves the cube that depends on the recomputed VarSet `Touched`.
- **A failure.** `_recomputeFeature` turns an exception or an error return into an entry in the recompute log and the `Error` bit, and returns 1. Recompute then adds the object and everything that uses it, recursively (`getInListEx(filter, true)`), to a filter, skips them, and carries on with the rest. Before `94c228973d` (2019), the first failure stopped the whole recompute.
- **State after a failure** (probe, FreeCAD 1.1.3 headless). The failed object is `Touched` and `Invalid`, and keeps the shape it had before: a `Part::Box` set to length 0 still measures 1000. Its users aren't run and aren't touched, so they show `Up-to-date` with shapes built from the old input: a `Part::Cut` of that box still measures 875. Once the failure is fixed, only the failed object and its users run (2 of 5 in the probe).
- **Abort.** A `Base::AbortException` from the progress dialog returns -1, which ends both passes at once. Objects not yet reached keep their old shapes and their touched bits. Nothing is rolled back. Aborting can be turned off (`CanAbortRecompute`, `b78f1a5111`).
- **Two passes.** An object still touched after the first pass gets a second, "to allow some form of dependency inversion" (`94c228973d`); one still touched after that is logged as an error (`84280330aa`).
- **Cycles.** Refused when a link or expression is set (`testIfLinkDAGCompatible`, `a1557d07da`; `testCyclicDependency`). A cyclic graph that gets through either throws `BadGraphError` (with `DepNoCycle`) or recomputes the acyclic part (`partialTopologicalSort`, `91bb7ed0c5`, #3185). `Document::checkOnCycle()` is a stub that returns false.
- **No parallelism.** Objects run one at a time. Async recompute (`cd1c8406a8`, 2026; on by default since `de7e90753f`) moves a whole recompute onto one background thread, still one object at a time, holding the GIL throughout. Requests are queued per document. Closing a document drops its queued requests and waits for one in flight (`CloseDocumentWaitsForInFlightAsyncRecompute`).

## Edge cases from FreeCAD's history

| Commit | What it fixed or added | In Silico |
|---|---|---|
| `283ab961b5` (2017), `6186a310d7` (2018) | Touched objects whose class didn't override `mustExecute()` weren't recomputed, "in a broken state"; `Prop_NoRecompute` made touching enforce recompute | No flags to forget: a feature reruns when its key changes (below) |
| `93f06f8256` (2018, #3410) | 0.17 recomputed more than 0.16 and was slower; split touched from must-recompute | The key decides, so an edit reruns exactly the features whose inputs changed |
| `94c228973d` (2019) | A failure skips its users and the rest carry on, instead of ending the recompute | The same, but the users are reported as blocked, not left showing stale shapes |
| `6a726f6c88` (2023), `aefe6e1790` (2020) | Report each failed object's own error, not "Recompute failed!" | One structured error per failed feature, in timeline order |
| `033242ead8` (2017), `0a2d8dc5ca` (2019) | A recompute called during a recompute is refused, then only logged | Features never start a recompute; a new edit supersedes the old one by generation |
| `75385d7f34` (2019), `01996d8f13` (2016) | Recompute ignored during undo and redo; `SkipRecompute` to batch edits | An undo is an edit: a new generation, through the same gate |
| `18355cdd25` (2013, #1019), `2c378a4f8c` (2013) | Crashes from a stale dependency list and from objects removed during recompute | Recompute reads an immutable snapshot of the document, so an edit can't change it under a running recompute |
| `91bb7ed0c5` (2017, #3185), `a1557d07da` (2015) | A cycle in part of the graph stopped every recompute; links that would make a cycle refused | The gate refuses any reference to a later feature (M2.2), so the timeline is already a topological order |
| `b0c2175f1d` (2025) | Fine-grained recompute: dependencies per property, so editing `VarSet.Length1` doesn't touch a cube using `Length2` (`testIndependent`) | When parameters arrive, a feature depends on the parameters its expressions name, not on the whole set |
| `e782c113e3` (2026) | Autosave blocked while a document is recomputing | Saving writes text, which never waits on recompute; M2.6 must write only fingerprints computed from the text being saved |

## What Silico does

- **Order is the timeline's.** A valid document references only earlier features, so its array order is a topological order. No sort, and no cycle handling. Recompute takes the gate's output as a type (`ValidDocument`, say), so an unvalidated document can't reach it.
- **The graph.** Today each solid is consumed once (D2), so the features form trees, one per body, and a feature's dependents are the single path to its body. The scheduler shouldn't rely on that: sketches (M3) and parameters will be used by several features. It works on any DAG of feature inputs.
- **Keys, not touched bits.** Each feature gets a key: a stable hash (no `HashMap` order, no random seed) of its type, version, and the values that reach the kernel, and of its inputs' keys. Not its ID, name, note, or place in the timeline. Recompute keeps the last results by key and reruns a feature only when its key is new. So an unchanged document runs nothing; a rename, a note, or reordering independent features runs nothing; and a value edit reruns that feature and the path below it. M2.5's cache stores the same results under the same keys (with the kernel version and platform added).
- **Keys per request.** A primitive is two kernel requests (make at the origin, then translate). Keying each request, the translate's key built from the make's, means a position edit reruns the translate and what uses it, not the make: what `Prop_NoRecompute` does for FreeCAD's placements.
- **Parallel branches.** A feature is submitted to the kernel when all its inputs are built. Ready features are submitted in timeline order, so the queue is deterministic, though nothing depends on it: each result is a function of its key alone. Independent branches run at once across the pool. Within one feature, requests run in sequence.
- **Failures, as SolidWorks.** A feature whose request fails is `Failed`, with the structured error naming it. Each feature that uses a failed or blocked feature is `Blocked`, naming the failed feature it waits on, and isn't run. Every other feature builds. Errors are listed in timeline order whatever order they arrived in, so the outcome doesn't depend on scheduling. No stale results: a failed or blocked feature has none, unlike FreeCAD, which shows the old ones as current.
- **What's kept.** A refusal that follows from the inputs (`kernel_failed`, `invalid_input`) is kept with the key, so it isn't retried until something changes. A failure that doesn't (`worker_crashed`, `worker_timeout`, `resource_exhausted`) isn't kept: the next recompute tries again. The pool never retries within one.
- **Generations and cancellation.** Every edit that passes the gate gets the next generation, and starts its recompute with `Kernel::advance`, which supersedes the older one: its waiting requests are dropped and its running ones stopped or discarded (`Outcome::Superseded`). A recompute builds its outcome aside and publishes it whole, only if its generation is still current. So a cancelled recompute leaves the previously published state as it was, never a mix. The generation is never written to the file or into a result.
- **Off the caller's thread.** Recompute returns at once with a handle; the caller waits, polls, or drops it (invariant 4). Spans per recompute and per feature, carrying the feature's ID.
- **The result.** Per feature: its key and outcome (the shape and history, or the error, or the feature it's blocked on). The bodies: the built solids no built feature consumes, in timeline order.
- **Not in M2.4:** partial recompute and a rollback marker (later, with the timeline UI), suppression (FreeCAD's `Freeze`), the disk cache (M2.5), fingerprints (M2.6).

## Tests

FreeCAD's C++ document tests (`tests/src/App/Document.cpp`) test only string hashers. The recompute tests are Python:

| Test | What it asserts | In Silico |
|---|---|---|
| `src/Mod/Test/Document.py` `DocumentRecomputeCases.testRecompute` | An 8-object graph with two roots and shared inputs (L5 used by L2 and L3, L1 with three inputs): sorted order puts users after inputs; recompute counts after enforcing each object: L5 4, L4 3, L5 4, L6 3, L2 2, L1 1, with exact per-object run counts | Ported on the scheduler's own graph (the document can't share a solid yet), with fake builds counting runs |
| `Document.py` `testNoRecompute`, `testNoRecomputeParent` | A touch that doesn't enforce recompute reruns the user, not the object | Ported as the position edit: one translate and the booleans below it, no make |
| `Document.py` `FeatureTestAttribute.testInvalidAttribute` | A failed object is `Invalid` and `Touched` | Ported as `Failed` with its error, and no result (deliberately not `Touched` with an old one) |
| `src/Mod/Test/Recompute.py` `testIndependent`, `testSimple`, `testChaining` | Per-property dependencies; partial recompute leaves users touched | With parameters (not on the roadmap yet); partial recompute with rollback |
| `Recompute.py` `testCyclicDependency`, `Document.py` `testCyclicDependencyOnPlacement` | A cyclic expression is refused | The gate's (M2.2), for references now and expressions later |
| `tests/src/App/AsyncRecompute.cpp` `CloseDocumentWaitsForInFlightAsyncRecompute` | Closing waits for the running recompute | A deliberate difference: dropping a recompute's handle or closing supersedes it, returns at once, and no result arrives after |

Silico's own, on `FakeKernel` unless marked:

- **Order:** for generated valid documents, every request's inputs were made by earlier calls (`calls()`).
- **Minimal:** a document of two independent cuts, one value edited: the calls are exactly that primitive's and the boolean above it. Unchanged, renamed, re-noted, or reordered: no calls. A position edit: a translate, no make.
- **Parallel:** two branches held (`hold`) are running together (`wait_running(2)`); a single chain never has more than one running (`most_running() == 1`).
- **Failure:** a refused feature is `Failed` with its error; its consumers are `Blocked` on it; every other feature is built; errors in timeline order. Fixing it reruns only it and its blocked consumers (FreeCAD's probe: 2 of 5).
- **Cancellation:** a held recompute, superseded by a newer generation, returns `Superseded`; the published state equals the one before; the newer one completes, and no call of the older generation starts after `advance`.
- **Determinism:** the same document under pool sizes 1, 2, and 8 and random release orders gives identical outcomes, byte for byte. On the real pool (OCCT), the same document twice gives identical B-rep bytes.
- **Crashes:** a feature failing with `worker_crashed` blocks its consumers and is retried by the next recompute of an unchanged document; one failing with `kernel_failed` isn't.

## Open questions

M2.4 is built on each recommended default; the maintainer reviews them with the rest of M2 at its end.


1. **When a boolean fails, are its base and tool bodies?** Recommended: yes. A failed or blocked feature consumes nothing, so the model shows what did build, as if the feature were suppressed. Alternative: they stay consumed, and the model shows neither.
2. **Is `Blocked` an error?** Recommended: an outcome naming the failed feature, not an error, so the error list has one entry per cause and the error contract (`tests/contract/`) doesn't change. Alternative: a new error code for it, with the maintainer.
3. **Results a cancelled recompute finished before it was superseded.** Recommended: keep them by key, in memory, since a key makes them right whatever the generation; they never reach published state. Alternative: discard everything from a superseded generation.
4. **Generations are per kernel, not per document.** Advancing for one document would supersede another's recompute on the same pool. Recommended: one open document per kernel in M2; the pool's generation becomes per document when several documents (or assemblies) share one.
5. **What the key leaves out.** Recommended: the ID, name, note, and timeline place, so two identical features share a result. If naming (M2.7) puts the feature's ID into element names, the ID joins the key of the naming step, not of the kernel request.
