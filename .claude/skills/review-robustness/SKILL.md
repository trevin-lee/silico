---
name: review-robustness
description: Adversarial review hunting crashes, hangs, nondeterminism, and bad errors in Silico. Covers panics on input paths, unsafe code and FFI misuse, kernel worker crash/timeout/stale-result handling, UI thread blocking, races and leaks, float and unit mistakes, and errors an agent can't act on. Use after changes to the core, kernel workers, IPC, the app, or the MCP server. Pass a diff range, branch, or paths; with no argument it reviews the current branch's changes.
context: fork
background: false
---

# Robustness review

**Review target:** $ARGUMENTS

First read `.claude/skills/adversarial-review/reviewer-rules.md` and follow it.

Silico's core promise over FreeCAD is that it doesn't crash, hang, or quietly produce different results on each run. Find where this change breaks that promise.

## Method

1. **Panics on input paths.** Treat four sources as untrusted: user input, agent (MCP) input, files, and kernel worker responses. A worker can die mid-message, return garbage, or never answer. On any path those reach, look for `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, slice indexing, integer overflow, `as` casts from negative or NaN values, division by zero, and normalizing a zero-length vector.

2. **Unsafe code and FFI.** `unsafe` outside FFI crates is a finding. In FFI code, check that C++ exceptions can't cross the boundary (a `cxx` function must declare `Result` if it can throw), that null OCCT handles are handled, and that ownership and lifetimes across the boundary are correct.

3. **Worker lifecycle.** Trace what actually happens when a worker:
   - segfaults or aborts partway through an operation,
   - hangs (is there a timeout, and does cancellation work?),
   - runs out of memory,
   - returns a result for a request that newer edits have superseded (is the stale result applied?),
   - restarts (what state is lost, and is it rebuilt?).

   Also check for zombie processes and leaked shared memory or temp files.

4. **Concurrency.** Look for anything on the egui update path that can block: synchronous kernel calls, IO, recompute, or a lock that someone else holds for a long time. Also check lock ordering, unbounded channels, and whether superseded recomputes get cancelled during interactive drags.

5. **Determinism.** Look for `HashMap` or `HashSet` iteration order leaking into results, names, or serialized output. Also parallel float reductions, and anything that depends on time, randomness, pointer addresses, thread scheduling, or OCCT's parallel mode. When you can, run the same operation twice and diff the outputs.

6. **Errors.** Look for errors that lose context: `map_err(|_| ...)`, `.ok()`, `let _ =` on a `Result`, or a string where a structured code belongs. Check that each error says which feature and which element failed, why, and what to try next. An agent should be able to recover from it without a human. Kernel operations need `tracing` spans that cross the worker boundary.

7. **Numbers.** Look for `==` on floats, inconsistent tolerances, degrees mixed with radians, mm mixed with m, and NaN or infinity spreading silently into geometry.
