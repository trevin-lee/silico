# Silico

Silico is a parametric CAD system: a Rust core, an egui/wgpu UI, and OpenCascade as the kernel. It's agent-native, meaning agents and humans are equal clients, and git-native, meaning models are diffable text. It ports FreeCAD's behavior into idiomatic Rust.

**Read [ARCHITECTURE.md](ARCHITECTURE.md) before structural work.** Its invariants are non-negotiable unless the maintainer agrees to change them.

**Status:** M0 (foundations) in progress; see [ROADMAP.md](ROADMAP.md). `silico-kernel` has the OCCT bridge (M0.4); the other crates are scaffolding.

## FreeCAD reference

- FreeCAD's source is a separate clone at `~/Git/FreeCAD`, outside this repository, with full history. It's the maintainer's fork, kept for reference and for upstream PRs: read it, but don't modify it from Silico work. If it's missing, run `git clone git@github.com:trevin-lee/FreeCAD.git ~/Git/FreeCAD`.
- [docs/freecad-map.md](docs/freecad-map.md) maps each Silico subsystem to FreeCAD's code, and shows how to mine FreeCAD's history and run it headless.
- **Port behavior, not lines.** Before porting a subsystem, write `docs/ports/<subsystem>.md` covering what it does, its edge cases, the bug fixes from FreeCAD's git history (with commit hashes), and which FreeCAD tests to port. Then implement from that note and those tests, in idiomatic Rust.
- Link PlaneGCS and OndselSolver as C++ in worker processes. Don't port them.

## Writing code

- No C++ in the app process. OCCT, PlaneGCS, and OndselSolver run in worker processes only.
- `unsafe` only in FFI crates. No `unwrap`, `expect`, or `panic!` on paths reachable from user, agent, file, or worker input. Return structured errors (code, feature, element, cause, suggestion). Workspace lints enforce this. Tests may unwrap inside `#[test]` functions and `#[cfg(test)]` modules, and integration-test files under `tests/` may allow those lints at their top. Never add lint allows anywhere else.
- Nothing that reaches results, names, or serialized output may depend on `HashMap` order, time, randomness, or thread scheduling. Use `BTreeMap` or `IndexMap`.
- Never block the UI thread.
- Every capability goes through the core command API, so the UI, MCP, and CLI all reach it.
- Instrument work with `tracing` spans.
- When a UX choice is unclear: **behavior follows SolidWorks, visuals and UI follow Fusion 360.** Never copy FreeCAD's UX.

## Testing

Tests stand in for line-by-line human review, so they have to be able to fail.

- Every behavior change comes with tests whose expected values come from an independent source: an analytic result, a hand calculation, or FreeCAD as an oracle.
- Never weaken, delete, or `#[ignore]` a test to get to green. `tests/oracle/`, `tests/golden/`, `tests/determinism/stress-reference.json`, and every crate's `tests/contract/` (the JSON shapes agents rely on) are protected: stop and ask before changing anything there.
- After implementing or porting something, run `/break-it` on it.
- Before merging, run `/adversarial-review`. It fixes confirmed findings and repeats rounds until the council approves, or it escalates to the maintainer. At the end of a milestone, run it with `--milestone`.

## Git

- One short-lived branch per task, off `main`. Squash-merge only after `/adversarial-review` approves. Never rewrite `main`.
- Conventional commits. The scope is the crate name without the `silico-` prefix: `feat(sketch): …`, `fix(kernel): …`, `docs: …`.
- Commit subjects say what the project gains. No version numbers in subjects.

## Skills

| Skill | Use it |
|---|---|
| `/adversarial-review` | Before merging: the review council, skeptics, fixes, and rounds until approved. Scales with the size of the change. |
| `/review-tests` `/review-robustness` `/review-quality` `/review-architecture` `/review-port` `/review-format` | To get one council member's review on its own |
| `/break-it` | After writing code: write and run hostile-input tests |

**Roadmap:** see [ROADMAP.md](ROADMAP.md).

## Commands

```bash
pixi run occt      # build OCCT with the deterministic recipe (about 3 minutes when inputs change;
                   # builds are shared between checkouts in ~/.cache/silico/occt, or $XDG_CACHE_HOME/silico/occt
                   # or $SILICO_OCCT_CACHE if set; delete it to start fresh)
pixi run test      # OCCT, then the libm shim tests, the OCCT guard and cache tests, and cargo nextest
pixi run lint      # fmt and clippy, including the libm shim crate
pixi run stress    # 605-case stress test, 10 runs, byte-identical to the committed reference
pixi run gate      # every check on the committed tree: workspace lints, cargo-deny, lint, test, one stress run
cargo mutants --in-diff <(git diff main...HEAD)
```

Run C++ builds through pixi so the pinned clang is used. `pixi run gate` runs every check on the committed tree and must pass before a merge. `silico check` and `silico fmt` arrive in M2.
