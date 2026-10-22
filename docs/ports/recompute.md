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

Built in M2.4 (`silico-core`'s `recompute`).

- **Order is the timeline's.** A valid document references only earlier features, so its array order is a topological order. No sort, and no cycle handling. Recompute takes only a `document::Checked`, a document the gate passed, so an unvalidated document can't reach it.
- **The graph.** Today each solid is consumed once (D2), so the features form trees, one per body. The scheduler doesn't rely on that: it works on the steps' inputs, so a step two features share (identical steps are one) is a diamond, as `testRecompute`'s are.
- **Keys, not touched bits.** Each kernel step has a key: SHA-256 of a domain line and a line saying what the step asks of the kernel, every number by its bits, and its inputs' keys. Not a feature's ID, name, note, version, or place in the timeline: two features asking the kernel for the same thing share a result, whatever their version (a version that changes what is asked changes the key; one that doesn't, needn't). Any change to what a step sends the kernel for the same values must change the domain line's number, since M2.5 keeps keys on disk; a golden key pins the spelling. An unchanged document runs nothing; a rename, a note, or reordering independent features runs nothing; a value edit runs that feature's steps and those below it.
- **Keys per request.** A primitive is two kernel requests (make at the origin, then translate, unless its position is the origin). The translate's key is built from the make's, so a position edit runs the translate and what uses it, not the make: what `Prop_NoRecompute` does for FreeCAD's placements.
- **Parallel branches.** A step goes to the kernel when all its inputs are built, from up to 64 threads, so independent branches overlap on the pool. The first steps go in timeline order; after that, steps go as their inputs finish, so the order they're sent in varies, but nothing depends on it: each result is a function of its key alone.
- **Failures, as SolidWorks.** A feature whose step fails is `Failed`, with the kernel's structured error naming the feature. Each feature that uses a failed or blocked feature is `Blocked`, naming the failure at the root (its base's, then its tool's), and isn't run. Every other feature builds. Errors are listed in timeline order. No stale results: a failed or blocked feature has none, unlike FreeCAD, which shows the old ones as current.
- **What's kept.** A refusal that follows from the values (`invalid_input`, `kernel_failed`, `invalid_result`) is kept with the key, so it isn't asked again until something changes: unlike FreeCAD, which tries every failed object again on each recompute (a failed object stays `Touched`). A failure that doesn't follow from the values (`worker_crashed`, `worker_unavailable`, `worker_timeout`, `resource_exhausted`, a protocol error) isn't kept: the next recompute tries again. OCCT running out of memory (`Standard_OutOfMemory`, `std::bad_alloc`) where the exception reaches Silico (a primitive, a move) is `resource_exhausted`, not `kernel_failed`, so it isn't kept either; but OCCT's booleans and fillets catch an OCCT `Standard_OutOfMemory` themselves (they catch `Standard_Failure`; a `std::bad_alloc` still reaches Silico) and report only that they failed (`BOPAlgo_AlertIntersectionFailed`, `BOPAlgo_AlertBuilderFailed`), which is `kernel_failed` and kept (and its validity check, which marks the shape invalid: `invalid_result`, kept), as any of their failures is (a decision for the maintainer at M2's end; M2.5's cache keeps no step's failure on disk, so it lasts a session at most). Results are kept while the last two published models' plans, or the newest recompute's, use them: going back to the model before (an undo) runs nothing, and a run of superseded recomputes holds no more than the newest one's work.
- **Generations and cancellation.** Every recompute gets the next generation and starts with `Kernel::advance`, which supersedes the older one: its waiting requests are dropped and its running ones stopped or discarded (`Outcome::Superseded`). A recompute builds its outcome aside and publishes it whole, only if no newer recompute started and it wasn't cancelled, so a superseded or cancelled one leaves the published model as it was, never a mix. The generation is never written to the file or into a result.
- **Off the caller's thread.** `start` returns at once with a `Run`; the caller waits, polls, or drops it (invariant 4). A span per recompute (its generation) and per step (its key, and the first feature it's a step of), at warning level, under the caller's span, so a warning during a step, the worker's own forwarded log lines included, traces back to the feature and the edit that started it (invariant 9). A step that panics (a bug) ends the recompute with an `internal` error naming the panic and the feature.
- **The result.** Per feature: its outcome (its last step's key and result, the shape and history; or the error; or the feature it's blocked on). The bodies: the built solids no built feature consumes, in timeline order.
- **Not in M2.4:** partial recompute and a rollback marker (later, with the timeline UI), suppression (FreeCAD's `Freeze`), the disk cache (M2.5), fingerprints (M2.6).

## Tests

FreeCAD's C++ document tests (`tests/src/App/Document.cpp`) test only string hashers. The recompute tests are Python:

| Test | What it asserts | In Silico |
|---|---|---|
| `src/Mod/Test/Document.py` `DocumentRecomputeCases.testRecompute` | An 8-object graph with two roots and shared inputs (L5 used by L2 and L3, L1 with three inputs): sorted order puts users after inputs; recompute counts after enforcing each object, with exact per-object run counts | Not ported as a graph: a document can't share a solid between features yet. Its diamond is `a_step_two_features_share_runs_once_and_each_feature_above_it_runs_once` (one step, two paths, each run once, and an edit runs it and both paths once), and exact call counts over generated timelines are the property test `recompute_matches_a_sequential_evaluator_and_runs_exactly_the_new_steps`. Port the graph when features can share inputs (sketches, M3) |
| `Document.py` `testNoRecompute`, `testNoRecomputeParent` | A touch that doesn't enforce recompute reruns the user, not the object | `a_position_edit_reruns_the_move_not_the_make` |
| `Document.py` `FeatureTestAttribute.testInvalidAttribute` | A failed object is `Invalid` and `Touched` | `a_failed_feature_blocks_those_using_it_and_every_other_builds`: `Failed` with its error, and no result (deliberately not `Touched` with an old one) |
| `src/Mod/Test/Recompute.py` `testRelabel` | A relabel touches no user | `an_unchanged_renamed_renoted_or_reordered_document_runs_nothing` |
| `Recompute.py` `testIndependent`, `testSimple`, `testChaining` | Per-property dependencies; partial recompute leaves users touched | With parameters (not on the roadmap yet); partial recompute with rollback |
| `Recompute.py` `testCyclicDependency`, `Document.py` `testCyclicDependencyOnPlacement` | A cyclic expression is refused | The gate's (M2.2), for references now and expressions later |
| `tests/src/App/AsyncRecompute.cpp` `CloseDocumentWaitsForInFlightAsyncRecompute` | Closing waits for the running recompute | A deliberate difference: dropping the engine or cancelling supersedes the recompute, returns at once, and nothing of it is published |

Silico's own, on `FakeKernel` unless marked (`tests/recompute.rs`, and `/break-it`'s `tests/adversarial_recompute.rs`):

- **Order:** every request's inputs were made by earlier calls (the example); generated timelines run exactly the steps a sequential evaluator says, each once (the property test).
- **Minimal:** a value edit runs that feature's steps and the boolean above, call for call; unchanged, renamed, re-noted, or reordered runs nothing; a position edit runs a translate, no make; identical steps run once.
- **Parallel:** two branches held are running together (`wait_running(2)`); a chain never has more than one running.
- **Failure:** a refused feature is `Failed` with its error; its consumers are `Blocked` on it, and theirs on the root; every other feature is built; a kept refusal isn't asked again, an edit is; a crash is tried again.
- **Cancellation:** a held recompute, superseded or cancelled, returns `Superseded`; the published model is the one before; no step of it starts after the cancel; the newer one publishes.
- **Kept results:** an undo to the model before runs nothing; a run of superseded recomputes keeps only the newest one's steps.
- **Determinism:** the same document under kernel sizes 1, 2, and 8, and with its steps released in different orders, gives the same model; on the real pool (OCCT), the same document twice gives identical B-rep bytes.
- **Tracing:** a step's span descends from its recompute and the caller's span, and names its feature.

## Open questions

M2.4 is built on each recommended default; the maintainer reviews them with the rest of M2 at its end.

1. **When a boolean fails, are its base and tool bodies?** Recommended: yes. A failed or blocked feature consumes nothing, so the model shows what did build, as if the feature were suppressed. Alternative: they stay consumed, and the model shows neither.
2. **Is `Blocked` an error?** Recommended: an outcome naming the failed feature, not an error, so the error list has one entry per cause and the error contract (`tests/contract/`) doesn't change. Alternative: a new error code for it, with the maintainer.
3. **Results a cancelled recompute finished before it was superseded.** Recommended: keep them by key, in memory, since a key makes them right whatever the generation; they never reach published state. Built with a bound: kept while the newest recompute's plan or the last two published plans use them, so a run of superseded recomputes holds no more than the newest one's work, and an undo to the model before runs nothing. Alternative: discard everything from a superseded generation.
4. **Generations are per kernel, not per document.** Advancing for one document would supersede another's recompute on the same pool. Recommended: one open document per kernel in M2; the pool's generation becomes per document when several documents (or assemblies) share one.
5. **What the key leaves out.** Recommended: the ID, name, note, and timeline place, so two identical features share a result. If naming (M2.7) puts the feature's ID into element names, the ID joins the key of the naming step, not of the kernel request.
