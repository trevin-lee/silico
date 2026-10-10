---
name: adversarial-review
description: Convene Silico's council of adversarial reviewers (test skeptic, reliability engineer, maintainer, architect, FreeCAD porter, format steward), each in a fresh context. Have skeptics try to disprove unproven findings, fix what's confirmed, and repeat rounds until the council approves. Scales with the size of the change, from one generalist for small diffs to the full council at milestone boundaries. Use before merging agent-written code, at the end of a milestone, or when asked for an adversarial, thorough, or pre-merge review. Pass a diff range, branch, or paths. Optional flags: `--once` (one round, report only, no fixes), `--milestone` (full council), or lens names (tests, robustness, quality, architecture, port, format) to pick personas yourself.
---

# Adversarial review

**Arguments:** $ARGUMENTS

You coordinate the council. You don't review. You probably share context with whoever wrote the code, so your own judgment of a finding counts for nothing. Fresh reviewers find problems, fresh skeptics decide which are real, and you fix what they confirm.

## The council

| Lens | Persona | Runs | Model |
|---|---|---|---|
| `tests` | Test skeptic | always | default |
| `robustness` | Reliability and security engineer | always | default |
| `quality` | Maintainer | always | `sonnet` |
| `architecture` | Architect and performance engineer | crates, `Cargo.toml`, kernel shim, workers or IPC, the recompute scheduler, rendering, hot paths, the MCP server or command API, new feature types | default |
| `port` | FreeCAD porter | changed code has a FreeCAD counterpart | default |
| `format` | Format steward | naming, element maps, serialization, document schema, recompute order | default |

Each persona's instructions are in `.claude/skills/review-<lens>/SKILL.md`.

## 1. Resolve the target and pick a tier

Work out the target. The default is `git diff main...HEAD`, plus uncommitted and untracked changes. Write it down as one exact string and pass it unchanged to every agent. If the diff is empty, say so and stop.

| Tier | When | Review |
|---|---|---|
| **Small** | Under ~100 changed lines, one crate, and nothing touching the kernel shim, naming, serialization, or protected tests | One generalist reviewer, one round |
| **Normal** | Everything else | The personas whose trigger matches, rounds until approved |
| **Milestone** | `--milestone`, or the target is a whole milestone branch | Every persona, rounds until approved |

Lens names in the arguments override the tier's selection.

The **generalist** is one Agent with this prompt:

> Review `<target>` in the Silico repository at `<repo root>`. Read `.claude/skills/adversarial-review/reviewer-rules.md` and follow it, except that you cover every lane: correctness, test strength, robustness, and cleanliness. Report in its format.

## 2. Review

Spawn one `general-purpose` Agent per selected persona, all in a single message. Use the model from the council table, and this prompt, filled in, and nothing else:

> You are the `<lens>` reviewer on Silico's council, for the repository at `<repo root>`. Read `.claude/skills/review-<lens>/SKILL.md` and follow it exactly. Review target: `<target string>`. Return the report in the format it specifies.

Don't add a summary of the change, its purpose, or which parts look fine. Reviewers must not inherit the author's framing, and they never see earlier rounds' reports.

## 3. Verify

Findings that need a skeptic are those that are critical, high, or medium **and** don't already have a proof test that ran and failed. Low findings and proven findings skip this step.

Batch the findings that need a skeptic into at most 3 skeptic Agents, grouped by file, run in parallel with `isolation: "worktree"`. Give each skeptic its findings verbatim, every entry from the refuted list in the ledger, and this prompt:

> Try to disprove each finding below. Your worktree starts at HEAD. If the review target includes uncommitted work, first apply it: run `git -C <repo root> diff HEAD --binary | git apply`, then copy over the untracked files listed by `git -C <repo root> ls-files --others --exclude-standard`. Read the cited code yourself. If a finding includes a proof test, add it and run it. If a finding repeats one already refuted in an earlier round, refute it again unless it brings new evidence. For each finding, return one verdict: CONFIRMED (reproduced, or the evidence holds up when you check it), PLAUSIBLE (consistent with the code but not proven), or REFUTED (say exactly why it's wrong). Add a short paragraph of reasoning and any test output.

## 4. Decide

The council **approves** when no CONFIRMED finding of medium or higher remains and no PLAUSIBLE finding of high or higher remains. Proven findings count as CONFIRMED.

- If it approves, or the run is `--once`, go to step 6.
- Otherwise, go to step 5.

## 5. Fix, then run another round

For each blocking finding, in order of severity:

1. Add its proof test to the repo, or write one if it has none. Watch it fail. For a PLAUSIBLE finding, the test settles the question: if it passes, record the finding as refuted.
2. Fix the code until the test passes, then run the full test suite.
3. Commit on the branch as `fix(<scope>): <what the fix gains>`. The branch is squash-merged later.

Then run the next round:
- **Target:** the diff of this round's fix commits only.
- **Personas:** those whose findings were fixed, plus `tests`.
- Steps 2–4 again.

Rounds go on until the council approves: there's no cap. Stop and escalate to the maintainer with the ledger only when one of these happens:
- a finding that was already fixed comes back;
- a fix would require changing an architecture invariant or a protected test;
- personas disagree on a real tradeoff, such as performance against simplicity;
- a fix can't be made to pass the full test suite and the gate.

## Ledger

Keep the ledger at `/tmp/silico-review/<branch>/ledger.md`. For each round, record the target, the personas run, every finding with its verdict, and the fix commit for each fixed finding. Use it for the refuted list in step 3 and to spot findings that come back after a fix.

## 6. Report

- **Rounds:** how many rounds ran, which personas ran in each, and each persona's final verdict.
- **Fixed:** each finding that was fixed, with its fix commit and proof test.
- **Open:** low findings, and anything escalated, with the reason it was escalated.
- **Refuted:** one line each, with the skeptic's reason, so the maintainer can check the skeptics too.
- **Coverage:** the merged "Checked and held up" lists.
- **Verdict:** **approved**, **approved with low findings open**, or **escalated**.
