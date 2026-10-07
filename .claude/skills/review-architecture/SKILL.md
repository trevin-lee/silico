---
name: review-architecture
description: Adversarial review against Silico's architecture invariants. Checks that OCCT stays in kernel workers, the C++ shim stays geometry-only, feature logic lives in the Rust core, the UI, MCP, and CLI are equal clients of one command API, everything is in one modeling workflow (no Part vs PartDesign split), and new capabilities are usable by agents. Use after changes that add features, cross crate boundaries, add MCP tools or UI actions, or touch the kernel shim. Pass a diff range, branch, or paths; with no argument it reviews the current branch's changes.
context: fork
background: false
---

# Architecture review

**Review target:** $ARGUMENTS

First read `.claude/skills/adversarial-review/reviewer-rules.md` and follow it. Its invariants list is what you review against, together with `ARCHITECTURE.md` if it exists.

Architecture problems are cheap to catch in review and expensive to undo later. Look for changes that wear down the boundaries the rest of Silico depends on.

## Method

1. **Dependency direction.** Read the `Cargo.toml` diffs and run `cargo metadata`. The core must not depend on egui, wgpu, the app, or the MCP crate. Only kernel crates may link OCCT. The protocol crate stays small, with few dependencies.

2. **Logic creeping into the shim.** Any C++ in the kernel shim beyond "run the OCCT operation, return the shape and its history" is a finding. That includes naming, feature logic, choosing between strategies, retries, caching, and validation. All of it belongs in Rust, where testing is cheap.

3. **Client parity.**
   - A new UI action must be a command in the core API that MCP and the CLI can also reach. Validation or defaults computed only in the UI are findings.
   - A new MCP tool must go through the command API, not around it.

4. **One modeling workflow.** Look for anything that brings back FreeCAD's Part vs PartDesign split: separate object kinds, separate trees, or features that only work in one mode. Primitives, booleans, and sketch-based features are all features in one tree, and a feature can target any body.

5. **Agent usability.** For each new capability, check that an agent can
   - discover it (an MCP tool with a clear description and schema),
   - see the result (query, measure, or render to PNG),
   - undo it (it runs inside a transaction), and
   - understand failures (a structured error that names the feature and the element).

   Each missing piece is a finding.

6. **The feature graph.** New feature types must declare all their dependencies so that invalidation, recompute order, and parallel recompute stay correct. Hidden dependencies, such as reading global state or another feature that wasn't declared, are findings.

7. **Link vs port.** PlaneGCS and OndselSolver are linked as C++ libraries, not rewritten. A Rust reimplementation of either is a finding unless an ADR or `ARCHITECTURE.md` decided to do it.
