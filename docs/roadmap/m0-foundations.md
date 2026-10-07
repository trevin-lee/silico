# M0 – Foundations

**Goal:** prove the riskiest architectural choices work together before building on them: a Rust core talking to a worker process that calls OCCT through `cxx`, with OCCT installed reproducibly and results checked against FreeCAD.

**Exit test:** `box_fillet_through_worker_matches_freecad` (M0.8) passes, and the council approves with `/adversarial-review --milestone`.

---

### M0.1 – Workspace and lints

**Builds:** a Cargo workspace with empty crates `silico-core`, `silico-protocol`, `silico-kernel`, and `silico-cli`. `rust-toolchain.toml` pins the toolchain. Shared lints live in `[workspace.lints]`: `unsafe_code = "forbid"` everywhere except `silico-kernel`, and clippy `unwrap_used`, `expect_used`, and `panic` set to deny outside tests. Also `rustfmt.toml`, and `[workspace.dependencies]` for shared versions.

**Tests:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, and `cargo nextest run` all pass on the empty workspace.
- Manual check, recorded in the PR: adding `unsafe {}` to `silico-core` fails to compile, and adding `.unwrap()` in non-test code fails clippy.

### M0.2 – pixi environment

**Builds:** a `pixi.toml` with OCCT `>=8.0,<8.1`, cmake, and a C++ compiler toolchain, plus pixi tasks `build`, `test`, and `lint`. A separate `oracle` environment provides FreeCAD for the oracle tests, using the same OCCT minor version. Locally on macOS, `/Applications/FreeCAD.app` is the fallback.

**Tests:**
- On a clean checkout, `pixi run test` installs everything and runs the workspace tests.
- `pixi run -e oracle freecadcmd -c "import Part; print(Part.OCC_VERSION)"` prints `8.0.x`.

**Risk:** conda-forge's FreeCAD build may not use OCCT 8.0. If it doesn't, the oracle environment pins whichever FreeCAD build matches, and this file records the decision.

### M0.3 – The local gate

**Builds:** `pixi run gate`, the check every branch must pass before it merges. Hosted CI is deferred: the repository is local-only, and the gate costs nothing to run.

The gate checks out the branch's **committed** tree into a fresh temporary worktree, so it can't depend on uncommitted files or local leftovers. There it runs:
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`;
- `cargo nextest run`, including the oracle tests once they exist;
- `cargo mutants --in-diff` against `main`;
- a check that `#[allow(unsafe_code)]` appears only in FFI crates.

A pre-commit hook in `.githooks/`, installed with `pixi run install-hooks`, runs fmt and clippy on every commit. `/adversarial-review` runs the gate first and stops if it fails.

**Tests (each a manual check, recorded in the PR):**
- The gate passes on a clean branch.
- A deliberately failing test fails the gate.
- Code that depends on an uncommitted file fails the gate, even though it builds in the working tree.
- A surviving mutant fails the gate.
- `#[allow(unsafe_code)]` added to `silico-core` fails the gate.

**Later:** when the repository moves to GitHub, the gate script becomes the CI job, run on a public repo or a self-hosted runner to avoid paying for minutes.

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
