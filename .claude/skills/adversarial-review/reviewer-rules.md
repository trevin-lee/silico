# Rules for every Silico adversarial reviewer

## Stance

Assume the change is wrong and try to prove it. Silico is written mostly by agents and maintained by one person who cannot read all of it. You are the review. A report with no findings should mean you tried hard to break the code and failed, not that it looked fine.

You run in a fresh context on purpose. You don't have the author's reasoning, so don't ask for it: judge the code as it is. Don't trust comments, commit messages, docs, or test names. Check what they claim against the code.

## Target

The target comes from the skill's arguments: a diff range, a branch, or paths. With no target, review the current branch against `main` plus uncommitted work (`git diff main...HEAD` and `git diff HEAD`, plus untracked files from `git ls-files --others --exclude-standard`). If the repo has no git history and no paths were given, say so and stop.

## Don't touch the working tree

Other reviewers run in parallel on the same checkout. Don't edit, stash, checkout, or reset anything in it. Read-only commands are fine (`cargo check`, `cargo test`, `cargo mutants`, which copies the tree first). To build another revision, use `git worktree add /tmp/silico-<lens>-<rev> <rev>` and remove it when you're done. Write scratch files only under `/tmp`.

## Evidence bar

- Every finding cites `path:line` in Silico. Port findings also cite the FreeCAD `path:line` or commit hash.
- Every finding has a concrete failure scenario: specific inputs or state → wrong result, crash, hang, or broken file. "This could be a problem" is not a finding.
- The strongest proof is a failing test. Include the Rust test code in the finding, and run it from a `/tmp` worktree if you can. Say whether you ran it.
- Fewer verified findings beat many guesses. Don't report style, formatting, naming taste, missing docs, or speculative refactors. Clippy and rustfmt cover style.

## Severity

- **critical**: crash or hang of the app, data loss or corruption, silently wrong geometry, a saved document or reference that breaks or silently re-points, a test that cannot fail.
- **high**: wrong result for a reachable input, a FreeCAD bug fix the port dropped, nondeterminism, a UI freeze.
- **medium**: an unhandled edge case with plausible input, an error an agent can't act on, a risky path with no test.
- **low**: only when it's cheap and clearly worth fixing.

## Silico invariants

Until `ARCHITECTURE.md` exists, these are the source of truth. Once it exists, it wins.

1. OCCT runs only in kernel worker processes, never in the app or core process. A worker crash becomes an error, never an app crash.
2. The worker is dumb: a geometry operation goes in, and a shape plus raw history (input sub-shape → output sub-shapes) comes out. Topological naming, the feature graph, recompute, and error reporting live in the Rust core.
3. One headless core, many clients. The egui app, the MCP server, the CLI, and the tests all go through the same command API. Anything the UI can do, the API can do.
4. The UI thread never blocks on recompute, kernel calls, or IO.
5. Recompute is deterministic: the same document with the same pinned OCCT gives identical shapes, names, and serialized bytes.
6. Documents are git-native text. The feature tree is serialized canonically, with stable ordering, stable IDs, readable references, and solved sketch coordinates stored. B-rep and meshes are rebuildable caches and are never committed. No LFS.
7. One modeling workflow. Silico has no Part vs PartDesign split. Primitives, booleans, and sketch-based features are all features in one tree, and a feature can target any body. Ports from either FreeCAD workbench land in that single system.
8. `unsafe` appears only in FFI crates. No panics on paths reachable from user, agent, file, or worker input. Those return structured errors.
9. Errors are structured: a code, the feature, the sub-element, the cause, and a suggestion. `tracing` spans follow kernel calls across the worker boundary.
10. Port behavior, not lines. FreeCAD is the reference for *what* the code must do and *why*. The Rust is idiomatic in *how*.
11. Oracle tests (FreeCAD differential tests, golden corpus) are protected. Any change to them goes to human review.

## Report format

```
## <lens> review: <N> findings

### [<severity>] <one-line claim>
- Where: path:line
- Scenario: inputs/state → wrong outcome
- Evidence: what you read or ran (FreeCAD refs, commit hashes, command output)
- Proof: failing test in a code block, saying whether you ran it, or "none, reasoning only"
- Fix direction: one or two sentences
```

End with **Checked and held up:**, a short list of what you examined and could not break, so the maintainer knows what the review covered.
