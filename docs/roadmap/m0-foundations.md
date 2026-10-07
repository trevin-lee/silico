# M0 – Foundations

**Goal:** prove the riskiest architectural choices work together before building on them: a Rust core talking to a worker process that calls OCCT through `cxx`, with OCCT installed reproducibly and results checked against FreeCAD.

**Exit test:** `box_fillet_through_worker_matches_freecad` (M0.8) passes, and the council approves with `/adversarial-review --milestone`.

---

### M0.1 – Workspace and lints

**Builds:** a Cargo workspace with empty crates `silico-core`, `silico-protocol`, `silico-kernel`, and `silico-cli`. `rust-toolchain.toml` pins the toolchain. Shared lints live in `[workspace.lints]`: `unsafe_code = "deny"`, which `silico-kernel` overrides at its crate root, and clippy `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`, `print_stdout`, and `print_stderr` set to deny. `clippy.toml` still allows unwrap, expect, and panic in `#[test]` functions and `#[cfg(test)]` modules. Helpers in integration-test files (`tests/*.rs`) aren't covered by that, so those files may start with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`, and only those files. `forbid` isn't usable in the workspace table, because no crate could then override it. The gaps this leaves are closed by the gate in M0.3: an `allow`, `expect`, `warn`, or `cfg_attr` lowering `unsafe_code` in a non-FFI crate, or a crate missing `[lints] workspace = true`. Also `rustfmt.toml`, and `[workspace.dependencies]` for shared versions.

**Tests:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, and `cargo nextest run` all pass.
- `silico_binary_runs`: the `silico` binary starts and exits 0. It's the workspace's first real test, so `nextest` never needs `--no-tests=pass`, and the gate never uses that flag: a run that collects zero tests always fails.
- Manual check, recorded in the merge commit: adding `unsafe {}` to `silico-core` fails to compile, and adding `.unwrap()` in non-test code fails clippy.

### M0.2 – pixi environment and the deterministic OCCT build

**Builds:**
- A `pixi.toml` that pins the toolchain: clang 21, libc++, cmake, and ninja. On macOS, linking uses Apple's linker, because lld 21 can't read the macOS 27 SDK.
- `third_party/occt/`: the pinned OCCT release (tag and sha256), the patch series, and a build script that fetches, verifies, patches, and builds OCCT's modeling modules with the recipe in ARCHITECTURE.md's *Deterministic kernel*. The `libm` math shim is part of this step.
- pixi tasks `occt`, `build`, `test`, and `lint`.
- A separate `oracle` environment that provides FreeCAD for the oracle tests. Locally on macOS, `/Applications/FreeCAD.app` is the fallback.

**Tests:**
- On a clean checkout, `pixi run test` installs the toolchain, builds OCCT, and runs the workspace tests.
- `occt_imports_no_platform_math`: every math function a built OCCT library imports from the system is on the exact-by-definition allowlist (`sqrt`, `floor`, `fmod`, `ldexp`, and the like). Anything else, such as `sin`, `powf`, or `__exp10`, fails. An allowlist, not a blocklist: the compiler can introduce calls nobody wrote, such as `pow(10, x)` becoming `exp10`.
- `stress_corpus_bit_identical`: the 605-case stress test from `experiments/deterministic-kernel/` gives byte-identical output across 10 runs, and matches the committed reference output.
- `pixi run -e oracle freecadcmd -c "import Part; print(Part.OCC_VERSION)"` runs and prints the version it uses. The oracle needs only results within tolerance, so its OCCT version is recorded, not required to match.

### M0.3 – The local gate

**Builds:** `pixi run gate`, the check every branch must pass before it merges. It runs on the Mac, and CI (M0.9) runs the same script on the workstation.

The gate checks out the branch's **committed** tree into a fresh temporary worktree, so it can't depend on uncommitted files or local leftovers. There it runs:
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`;
- `cargo nextest run`, including the oracle tests once they exist;
- `cargo mutants --in-diff` against `main`;
- **forbid `unsafe` outside FFI crates:** `cargo check --workspace --all-targets` with a `RUSTC_WORKSPACE_WRAPPER` script that appends `-F unsafe_code` unless `$CARGO_PKG_NAME` is an FFI crate. A lint forbidden on the command line can't be lowered by any attribute (`allow`, `expect`, `warn`, `cfg_attr`). The wrapper applies it per crate because `RUSTFLAGS` would also hit FFI crates built as dependencies, and would replace any `rustflags` in `.cargo/config.toml`;
- **forbid panics and platform math in non-test code:** `cargo clippy --workspace --lib --bins -- -D warnings -F clippy::unwrap_used -F clippy::expect_used -F clippy::panic -F clippy::todo -F clippy::unimplemented -F clippy::dbg_macro -F clippy::print_stdout -F clippy::print_stderr -F clippy::disallowed_methods`. Test targets aren't compiled in this pass, so test files keep their exemption. Build scripts are compiled, so `build.rs` files return `Result` from `main` and report through `cargo:warning=`, never `expect`, `panic!`, or `eprintln!`;
- **cover every configuration:** the unsafe and panic passes run twice, once as above and once with `--release --all-features`, so code behind `cfg(not(debug_assertions))` or an optional feature is checked too. A source check rejects `cfg(clippy)` and `cfg(not(clippy))`, which would hide code from clippy entirely;
- a check that every member crate's `Cargo.toml` has `[lints] workspace = true`.

A pre-commit hook in `.githooks/`, installed with `pixi run install-hooks`, runs fmt and clippy on every commit. `/adversarial-review` runs the gate first and stops if it fails.

**Tests (each a manual check, recorded in the merge commit):**
- The gate passes on a clean branch.
- A deliberately failing test fails the gate.
- Code that depends on an uncommitted file fails the gate, even though it builds in the working tree.
- A surviving mutant fails the gate.
- Each of `#[allow(unsafe_code)]`, `#[expect(unsafe_code)]`, `#[warn(unsafe_code)]`, and `#[cfg_attr(all(), allow(unsafe_code))]` on an `unsafe` block in `silico-core` fails the gate.
- With `silico-kernel` added as a dependency of `silico-cli`, the clean tree still passes the gate.
- Each of `#[expect(clippy::unwrap_used)]`, `#[allow(clippy::restriction)]`, and `#[cfg_attr(all(), allow(clippy::panic))]` on non-test code in `silico-core` fails the gate.
- A new crate without `[lints] workspace = true` fails the gate.
- Each of these in `silico-core` fails the gate: `.unwrap()` under `#[cfg(not(debug_assertions))]`; `.unwrap()` under an optional feature; `.unwrap()` under `#[cfg(not(clippy))]`; `#[allow(clippy::disallowed_methods)]` on a function calling `x.exp()`.
- `platform_math_banned`: calling `x.sin()`, `f64::powi(x, 2)`, or `.map(f64::tan)` in `silico-core` fails the gate. This also catches a mistyped path in `clippy.toml`, which clippy accepts silently.
- Long tests (the stress and oracle suites) have their own longer timeout in `.config/nextest.toml`, so the 2-minute hang limit applies only to ordinary tests.

### M0.4 – The cxx bridge

**Builds:** in `silico-kernel`, a C++ shim and a `cxx` bridge. `build.rs` finds OCCT inside the pixi environment. Two functions: `make_box(dx, dy, dz) -> Result<UniquePtr<Shape>>` and `volume(&Shape) -> f64`. Every shim function catches `Standard_Failure` and `std::exception` and turns them into Rust `Err`, so no exception ever crosses the bridge.

**Tests:**
- `box_volume_is_exact`: a 10 × 20 × 30 box has volume 6000, within relative 1e-9 (analytic).
- `box_zero_dimension_is_err`: `make_box(0, 20, 30)` returns `Err` and does not abort the process.
- `box_negative_and_nan_are_err`: negative and `NaN` dimensions return `Err`.
- `exception_becomes_err`: a test-only shim function that throws `Standard_Failure` returns `Err` containing OCCT's message.

### M0.5 – Error type and tracing

**Builds:** the structured error type in `silico-core`. It carries a stable `code` enum, plus optional `feature`, `element`, `cause`, `suggestion`, and `candidates`. It implements `Display` for humans and `serde::Serialize` for MCP. `tracing` is initialized in `silico-cli`, with JSON output available.

**Tests:**
- `error_json_shape`: snapshot test of the serialized error. This file is the contract with agents, and changing it is deliberate.
- `error_codes_are_stable`: snapshot of every code and its string form, so a code can't be renamed or renumbered silently.
- `display_names_feature_and_element`: the human-readable message includes the feature, the element, and the suggestion.

### M0.6 – Protocol and framing

**Builds:** in `silico-protocol`, the types `Request { id, generation, op }` and `Response { id, result }`. Messages are length-prefixed frames, encoded with serde and `postcard`, with a maximum frame size. The decoder works incrementally over any `Read`.

**Tests:**
- `roundtrip_every_op`: every request and response type survives encode → decode unchanged.
- `decoder_never_panics` (proptest, at least 10,000 cases): arbitrary bytes produce `Ok` or a decode error, never a panic.
- `truncated_frame_is_error`: cutting a valid frame at every byte offset gives a clean error.
- `oversized_frame_rejected`: a length prefix above the maximum is rejected before any allocation.
- A `cargo fuzz` target for the decoder, run for at least 5 minutes before merging.

### M0.7 – Worker process and client

**Builds:** the `silico-kernel-worker` binary, which reads requests from stdin and writes responses to stdout, with logs on stderr. In `silico-core`, a client that spawns one worker, sends a request, and waits with a timeout. When a worker crashes or times out, the client kills it, reaps the process, and returns a structured `WorkerCrashed` or `WorkerTimeout` error. Behind a cargo feature `test-ops`, the worker has extra operations for testing: `debug_sleep`, `debug_abort`, `debug_segfault`, and `debug_garbage` (writes invalid bytes).

**Tests:**
- `box_volume_through_worker`: `make_box` then `volume` through the worker returns 6000.
- `segfault_becomes_error`: `debug_segfault` gives `WorkerCrashed`, and the test process keeps running.
- `abort_becomes_error`: same, for `debug_abort`.
- `hang_times_out`: `debug_sleep(60s)` with a 1 s timeout gives `WorkerTimeout` within 2 s, and the worker's process ID no longer exists.
- `garbage_reply_is_error`: `debug_garbage` gives a protocol error and the worker is killed.
- `no_zombies`: after 100 crash and timeout cycles, the test process has no child processes left.

### M0.8 – Fillet and the FreeCAD oracle

**Builds:** a shim function `fillet(shape, edge_indices, radius)`. Edges are indexed using OCCT's `TopExp::MapShapes` order. Also a shape summary with volume, area, face/edge/vertex counts, and validity from `BRepCheck_Analyzer`. And a minimal oracle helper: it runs a FreeCAD Python script with `freecadcmd` and parses the JSON it prints.

**Tests:**
- `box_fillet_analytic`: a 10 × 20 × 30 box with one edge of length 30 filleted at r = 2 gives:
  - volume 5974.24778 (6000 − (4 − π)·30), within relative 1e-6;
  - area 2172.53097 (2200 − 120 + 30π − 2(4 − π)), within relative 1e-6;
  - 7 faces, 15 edges, 10 vertices;
  - a valid shape.
- `box_fillet_through_worker_matches_freecad` (**the M0 exit test**): the same operation through the worker and through FreeCAD gives the same volume and area within relative 1e-9, the same topology counts, and both shapes are valid.
- `fillet_too_large_is_structured_error`: r = 11 on an edge whose adjacent face is 10 wide returns a `FilletFailed` error naming the edge. If FreeCAD succeeds on this input, record it as a spec question instead of asserting.

### M0.9 – CI on the workstation

**Lands whenever the maintainer's runner is ready. It doesn't block the M0 exit test.**

**Builds:** GitHub Actions on a private GitHub repository. Jobs run only on the maintainer's self-hosted runner: the Ryzen 9950X3D k3s workstation, x86-64 Linux, using Actions Runner Controller (ARC).
- Every job's `runs-on` targets the self-hosted runner's labels and never a GitHub-hosted runner. If the workstation is offline, jobs queue instead of spending paid minutes. GitHub fails a job that has been queued for more than 24 hours, so after a long outage those runs need a re-run.
- The repository stays private while CI caches persist between jobs. If it ever goes public, fork pull requests must require approval before running, because they could poison the shared caches or run code on the workstation.
- Runner pods are ephemeral, and persistent volumes cache pixi packages, the cargo registry, and compiled output (sccache).
- On every push, the workflow runs `pixi run gate`, the same script as the local gate, plus the jobs too heavy for the laptop: full mutation testing and the oracle suite.
- Nightly, it runs long fuzzing and the generated oracle cases.

**Needs first:** the private GitHub repository and the remote. The maintainer creates these, or approves creating them.

**Tests:**
- A push to a branch triggers the gate, and its status shows on the commit.
- A deliberately failing test fails the run.
- No workflow job can select a GitHub-hosted runner. A check in the gate rejects any `runs-on` value without the self-hosted labels, and a workflow with `runs-on: ubuntu-latest` fails the gate (manual check).
- `cross_platform_determinism`: the stress corpus, the golden corpus, and every oracle case are bit-identical between the Mac and the workstation: shapes, names, summaries, and saved documents. Any difference fails the run and is treated as a bug.
