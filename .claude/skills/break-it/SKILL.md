---
name: break-it
description: Write adversarial tests that try hard to break Silico code with hostile, degenerate, and malformed inputs (NaN and tolerance-edge numbers, self-intersecting sketches, impossible fillets, corrupt files, garbage MCP calls, killed workers), run them, and keep every test that exposes a crash, hang, wrong result, or bad error. Use after implementing or porting a feature, parser, solver, or API; when asked to stress, fuzz, harden, or "throw garbage at" code; or before trusting code that only has happy-path tests. Pass a function, module, crate, feature name, or paths; with no argument it targets the current branch's changes.
context: fork
background: false
---

# Break it

**Target:** $ARGUMENTS

Read `.claude/skills/adversarial-review/reviewer-rules.md` for severity levels and Silico's invariants. Its "don't touch the working tree" rule doesn't apply here, because writing tests is your job. Never edit the code under test.

Your goal is to make the target crash, hang, return wrong geometry, or fail with a useless error. Happy-path tests already exist. You write the tests a malicious user, a confused agent, or a corrupted file would effectively write. A run that breaks nothing is fine, but only if the attempt was serious.

## 1. Map the attack surface

Read the target and its existing tests. List every input it accepts: function arguments, document state, sketch geometry, file bytes, MCP JSON, worker responses, and timing (concurrent calls, cancellation). Write down what the code assumes about each input, whether that's stated in a comment or implied by the code (an index into a slice, a division, a vector that gets normalized). Every assumption is a target.

## 2. Collect hostile inputs

- **From the catalog.** Go through [catalog.md](catalog.md) and pick every category that applies to this target. Try every applicable category, not a sample.
- **From FreeCAD's crash history.** Real users already found the inputs that break this kind of code. Find the FreeCAD counterpart, then run `git -C FreeCAD log --oneline -i --grep='crash\|segfault\|hang\|freeze\|fix' -- <file>` and `git -C FreeCAD show <hash>` to see which inputs triggered each fix. Turn each one into a test.
- **From the code itself.** Every branch, tolerance, and early return has inputs on either side of it and exactly at it. Test all three.

## 3. Decide what counts as broken

A test passes only if the target either returns a **correct result**, checked independently, or returns a **specific structured error**: the right error code, the feature and element named, and an actionable message. Everything else is a break:

| Break | How to detect it |
|---|---|
| Panic or crash | Test panics; a kernel worker dies and the error isn't handled cleanly |
| Hang | Exceeds the time limit. Run under `cargo nextest` with a slow timeout, or wrap the call in a thread with `recv_timeout`. |
| Invalid geometry returned as `Ok` | The shape fails validity checks, has a non-closed shell, or has zero or negative volume |
| Wrong result | Doesn't match an analytic answer or FreeCAD's answer for the same input |
| `Ok` on nonsense | For example, a zero-length pad that returns an empty shape instead of an error |
| Bad error | A generic error, a string-only error, missing context, or the wrong code |
| Nondeterminism | Two runs of the same input give different shapes, names, or bytes |
| Corruption | A failed operation leaves the document, undo stack, or worker in a broken state. After every expected failure, check that the next valid operation still works. |

When the correct behavior is unclear (should a 0° revolve be an error?), run the same input in FreeCAD with `/Applications/FreeCAD.app/Contents/Resources/bin/freecadcmd` and a script under `/tmp`. If FreeCAD's answer is also bad, list it as a **spec question** in the report instead of guessing.

## 4. Write and run the tests

Use three kinds of test, depending on what the target is:

- **Targeted tests** for each specific hostile input. Name each test after its input, for example `fillet_radius_exactly_half_edge_length` or `sketch_bowtie_self_intersecting_profile`.
- **Property tests** (proptest) for geometry and solver code. Bias the strategies toward edge values: zeros, tolerance boundaries, huge and tiny scales, near-parallel and near-coincident geometry. Plain uniform random floats almost never hit them. Use at least 1,000 cases.
- **Fuzz targets** (`cargo fuzz`) for anything that parses bytes: the file format, IPC messages, MCP requests, STEP import. Run each one for at least a few minutes. Save every crashing input as a regression fixture and add a test that loads it.

Put targeted and property tests in `<crate>/tests/adversarial_<target>.rs` and fuzz targets in `fuzz/`. Run everything. Minimize each failing case to the smallest input that still breaks, and merge failures that share a root cause.

Tests that break the code stay in the suite, **failing**, with a `// BREAKS:` comment that says what goes wrong. Don't use `#[ignore]`: the red test is the bug report and it turns green when the bug is fixed. Tests that pass also stay, as regression protection.

## 5. Grow the catalog

When you find a new kind of hostile input that the catalog doesn't cover, add it to [catalog.md](catalog.md) under the right heading, in one line. Future runs get better that way.

## Report

```
## break-it: <target> — <N> breaks, <M> survived

### [<severity>] <one-line description of the break>
- Input: minimized input
- Got: what happened (panic message, wrong value, timing, error text)
- Expected: correct result or the specific error, and how you know (analytic, FreeCAD, invariant)
- Test: path::test_name (failing)

### Spec questions
- <input>: Silico does X, FreeCAD does Y, and neither is clearly right

### Survived
Catalog categories tried with no break, and the count of passing tests added.
```
