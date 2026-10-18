# Silico

Silico is a parametric CAD system: a Rust core, an egui/wgpu UI, and OpenCascade as the kernel. It's agent-native, meaning agents and humans are equal clients, and git-native, meaning models are diffable text. It ports FreeCAD's behavior into idiomatic Rust.

**Read [ARCHITECTURE.md](ARCHITECTURE.md) before structural work.** Its invariants are non-negotiable unless the maintainer agrees to change them.

**Status:** M0 (foundations) and M1 (the kernel layer) are built: primitives, history, booleans, extrude and revolve, fillet and chamfer, the worker pool, display meshes with surface normals, the FreeCAD oracle harness, and hardening (a determinism sweep, refusing non-finite geometry, a corpus of real parts), closed by the milestone review. M2 (document and naming) is under way: M2.0, refusals as data (the worker says what it refused, and which elements, as data, and the core words it), M2.1, the document's types read and written exactly as canonical JSON, M2.2, the validation gate (IDs, references, consumed solids, and the kernel's value ranges), M2.3, the kernel trait and an in-memory fake kernel for core tests, M2.4, recompute (keyed steps, parallel branches, failures as SolidWorks, generations), M2.5, the rebuild cache (each step's result on disk, keyed by its step, the kernel's identity, and the platform), M2.6, fingerprints (each feature's measures stored with it, drift as an error), M2.7, the element map (every element named from history, as FreeCAD names it, in Silico's own form), M2.8, references by name (fillets and chamfers in the document, their edges by name, found again after upstream edits or failing loudly), and M2.9, the CLI (`silico check`, `fmt`, `show`, and a `git diff` driver), are built; the M2 milestone review is next. See [ROADMAP.md](ROADMAP.md) and the walkthrough, [docs/walkthrough/m0.md](docs/walkthrough/m0.md). What each crate holds now:

- `silico-kernel`: the OCCT bridge (C++ through `cxx`), the kernel worker binaries, and `silico-brep-convert` (OCCT text B-reps to Silico's, for the corpus). The only crate that links OCCT.
- `silico-protocol`: the messages between the core and workers, their framing, and the rules for valid primitive values (`Primitive::check`), profiles (`Profile::check`), and chamfer sizes (`ChamferSize::check`); and each history's result topology (`Topology`), which naming reads.
- `silico-core`: the structured `Error`; the worker client (`worker::Worker`), which starts workers and turns crashes, hangs, and bad replies into errors, each with advice that fits how the worker ended; typed kernel requests (`kernel::KernelRequest`: primitives, extrude, revolve, fillet, chamfer, translate, booleans, summarize, measure as FreeCAD does, tessellate), which check values and replies so nothing above them handles protocol messages, run on one worker (`kernel::KernelWorker`) or, through the `kernel::Kernel` trait, a pool of them (`pool::KernelPool`, which replaces workers that crash or hang, and drops or stops work of superseded generations) or an in-memory fake for tests (`fake::FakeKernel`, behind the `fake` feature: symbolic shapes whose bytes say what made them, every call recorded); and the document's types (`document`: read exactly from strict JSON by Silico's own reader, written in canonical form, with a JSON Schema, and checked by the validation gate, `document::validate`, into `document::Checked`); recompute (`recompute::Recompute`: a checked document rebuilt on a kernel, step by keyed step, in the background); and the rebuild cache (`cache::Cache`, `.silico-cache/`: each step's result on disk, sealed with a secret of the user's); and fingerprints (`document::Fingerprint`, each feature's measures, with `Model::drift` and `Model::fingerprinted`); and topological naming (`naming`: each element's `Name`, how it came about, in Silico's form or FreeCAD's, and each solid's `ElementMap`, given for each built feature by `Model::element_map`, read back by `Name::parse`); and references by name (fillets and chamfers whose `edges` are names, found in their base's element map when they rebuild, or each broken one a `reference_broken` error with candidates, never re-pointed); and the first of the core's commands (`command`: gate, rebuild, check, format, outline; and `file`, a document's file read and saved safely), which every client runs. The rest of the command API comes later.
- `silico-worker`: what every worker process is: the inherited socket, the request loop, logging, and the floating-point trap checks, shared by the kernel and sketch workers (its `unsafe`, for inherited descriptors only, is ARCHITECTURE.md's one exception).
- `silico-cli`: the `silico` command: `check`, `fmt`, `show`, and `textconv` for `git diff` (docs/cli.md), each a thin client of the core's commands (`silico_core::command`).
- `silico-view`: a debug viewer that shows the kernel's parts in Rerun, until the app exists; its library logs any mesh (faces shaded by the kernel's normals, labelled edges) for tests and scripts.
- `silico-oracle`: the FreeCAD oracle harness. Each case in `tests/oracle/` (protected) is built through a kernel worker and through headless FreeCAD and compared: volume, area, and center of mass measured as FreeCAD measures them, the tight bounding box, topology counts, and validity, to a tolerance set by what the shape is made of.

## FreeCAD reference

- FreeCAD's source is a separate clone at `~/Git/FreeCAD`, outside this repository, with full history. It's the maintainer's fork, kept for reference and for upstream PRs: read it, but don't modify it from Silico work. If it's missing, run `git clone git@github.com:trevin-lee/FreeCAD.git ~/Git/FreeCAD`.
- [docs/freecad-map.md](docs/freecad-map.md) maps each Silico subsystem to FreeCAD's code, and shows how to mine FreeCAD's history and run it headless.
- **Port behavior, not lines.** Before porting a subsystem, write `docs/ports/<subsystem>.md` covering what it does, its edge cases, the bug fixes from FreeCAD's git history (with commit hashes), and which FreeCAD tests to port. Then implement from that note and those tests, in idiomatic Rust.
- Link PlaneGCS and OndselSolver as C++ in worker processes. Don't port them.

## Writing code

- No C++ in the app process. OCCT, PlaneGCS, and OndselSolver run in worker processes only.
- `unsafe` only in FFI crates. No `unwrap`, `expect`, or `panic!` on paths reachable from user, agent, file, or worker input. Return structured errors (code, feature, elements and their total, parameter, cause, suggestion). Workspace lints enforce this. Tests may unwrap inside `#[test]` functions and `#[cfg(test)]` modules, and integration-test files under `tests/` may allow those lints at their top. Never add lint allows anywhere else.
- Nothing that reaches results, names, or serialized output may depend on `HashMap` order, time, randomness, or thread scheduling. Use `BTreeMap` or `IndexMap`.
- Never block the UI thread.
- Every capability goes through the core command API, so the UI, MCP, and CLI all reach it.
- Instrument work with `tracing` spans.
- When a UX choice is unclear: **behavior follows SolidWorks, visuals and UI follow Fusion 360.** Never copy FreeCAD's UX.

## Testing

Tests stand in for line-by-line human review, so they have to be able to fail.

- Every behavior change comes with tests whose expected values come from an independent source: an analytic result, a hand calculation, or FreeCAD as an oracle.
- Never weaken, delete, or `#[ignore]` a test to get to green. `tests/oracle/`, `tests/golden/`, `tests/determinism/stress-reference.json`, `tests/determinism/oracle-reference.json`, and every crate's `tests/contract/` (the JSON shapes agents rely on) are protected: stop and ask before changing anything there.
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
pixi run view      # the kernel's parts (a box, fillets, the primitives) in Rerun, edges labelled Edge1, ...
pixi run oracle    # every case in tests/oracle compared with FreeCAD
pixi run oracle-sweep  # 500 generated cases compared with FreeCAD (-- --seed S for others)
pixi run corpus    # real parts (NIST's STEP files, downloaded once) compared with FreeCAD
pixi run mutants -- --in-diff /tmp/change.diff --output /tmp
                   # cargo-mutants on the committed tree, in the gate's worktree (tests/mutants.sh), never
                   # in a checkout: every workspace test for each mutant (.cargo/mutants.toml), but the
                   # few that hold to a wall clock, which fail a loaded baseline. Arguments go to cargo
                   # mutants, paths absolute (git diff main...HEAD > /tmp/change.diff first; -p to
                   # narrow it). CI shards it over the runners (.github/workflows/mutants.yml)
```

Run C++ builds through pixi so the pinned clang is used. `pixi run gate` runs every check on the committed tree and must pass before a merge. `silico check FILE` checks a document and `silico fmt FILE` writes it in canonical form with its fingerprints (docs/cli.md).
