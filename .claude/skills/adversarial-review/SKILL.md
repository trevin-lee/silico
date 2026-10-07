---
name: adversarial-review
description: Run Silico's adversarial reviewers in parallel, each in a fresh context, then have independent skeptics try to disprove every finding before reporting. Use before committing or merging agent-written code, after a porting session, or when asked for an adversarial, thorough, or pre-merge review. Pass a diff range, branch, or paths, optionally followed by lens names (port, tests, robustness, format, architecture) to run only those. With no argument it reviews the current branch plus uncommitted work and picks lenses from what changed.
---

# Adversarial review

**Arguments:** $ARGUMENTS

You coordinate. You don't review. You probably share context with whoever wrote the code, so your own judgment of a finding counts for nothing here. Fresh reviewers find the problems and fresh skeptics decide which ones are real.

## 1. Resolve the target

Work out the review target from the arguments. The default is `git diff main...HEAD`, plus uncommitted and untracked changes. Write it down as one exact string, such as a commit range or a path list, and pass that string to every agent unchanged. If the diff is empty, say so and stop. If the repo has no git history and no paths were given, ask for paths.

## 2. Pick lenses

If the user named lenses, run exactly those. Otherwise:

| Lens | Run when |
|---|---|
| `tests` | always |
| `robustness` | always |
| `port` | changed code has a FreeCAD counterpart (sketcher, features, naming, expressions, assembly, import/export) or mentions FreeCAD |
| `format` | changes touch naming, element maps, serialization, document schema, or recompute order |
| `architecture` | changes touch `Cargo.toml`, crate boundaries, the C++ kernel shim, MCP tools, UI actions, or add a feature type |

## 3. Review in parallel

Spawn one `general-purpose` Agent per lens, all in a single message. Use this prompt, filled in, and nothing else:

> You are the `<lens>` reviewer for Silico, the repository at `<repo root>`. Read `.claude/skills/review-<lens>/SKILL.md` and follow it exactly. Review target: `<target string>`. Return the report in the format it specifies.

Don't add your own summary of the change, what it's for, or which parts look fine. Reviewers must not inherit the author's framing.

## 4. Try to disprove every finding

Collect the critical, high, and medium findings. Group them by file into at most 6 batches and spawn one skeptic Agent per batch, in parallel, with `isolation: "worktree"`. Give each skeptic the findings verbatim and this prompt:

> Try to disprove each finding below. Your worktree starts at HEAD. If the review target includes uncommitted work, first apply it: run `git -C <repo root> diff HEAD --binary | git apply`, then copy over the untracked files listed by `git -C <repo root> ls-files --others --exclude-standard`. Read the cited code yourself. If a finding includes a proof test, add it and run it. For each finding, return one verdict: CONFIRMED (reproduced, or the evidence holds up when you check it), PLAUSIBLE (consistent with the code but not proven), or REFUTED (say exactly why it's wrong). Add a short paragraph of reasoning and any test output.

Low findings skip this step and are reported as unverified.

## 5. Report

- Rank findings by severity, CONFIRMED before PLAUSIBLE. Include the proof test for every confirmed finding.
- List REFUTED findings at the end, one line each with the skeptic's reason, so the maintainer can check the skeptics too.
- Merge each reviewer's "Checked and held up" list into one compact coverage section.
- Finish with one verdict: **block**, **fix then merge**, or **clean**.

Don't fix anything unless asked. If asked to fix a finding, commit its proof test to the repo first, watch it fail, then fix the code until it passes.
