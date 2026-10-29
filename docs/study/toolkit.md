# The toolkit: egui, `eframe`, `egui_tiles`, `egui_kittest`, and `re_ui`

A study note for M5's design (M5.0), for open decision 11 (the toolkit) and decision 7 (how the app is tested) in `docs/roadmap/m5-app.md`. It covers:

- the versions `re_ui` 0.38.1 pins;
- whether `re_ui`'s dependency tree passes Silico's `cargo-deny` and whether anything in it reaches the network;
- whether Silico should take `re_ui` or only its style constants;
- whether `egui_kittest` can drive the app through its accessibility tree.

It describes what was found. The decisions stay with M5.0 and the maintainer.

**Sources.**

- The published crates at the versions named, read from Cargo's registry. Paths are cited as `crate-version/path`.
- Silico's `deny.toml` and `cargo-deny` 0.20.2 (pixi's), at `main` (`837347ac`).
- Three scratch crates outside the repository, each resolved against crates.io and checked with that `deny.toml`. A fourth, a test, drove a UI with `egui_kittest`. All are described under *Method* and were removed when this note was written.
- Read on 2026-10-27.

## In brief

- **The versions.** `re_ui` 0.38.1 pins:
  - egui and `eframe` 0.36.2;
  - `egui_kittest` 0.36.2;
  - `egui_tiles` 0.17.1, as a development dependency.
  - `eframe` 0.36.2's `egui-wgpu` uses wgpu 30, as `re_renderer` 0.38.1 does, so one wgpu serves the viewport and the UI.
- **What `re_ui` adds.**
  - It brings 279 crates more than the toolkit needs on its own (657 against 390). Among them are Rerun's data model, Arrow, a gRPC and HTTP server and client stack (tonic, hyper, axum, h2), rustls, and an HTTP client (ureq).
  - Those crates are already in Silico's lock file, through `silico-view`'s dependency on `rerun`, and already pass the gate's `cargo deny check bans licenses sources`. What's new is linking them into the app's binary: its size, and code it doesn't use.
  - Under Silico's `deny.toml`, `re_ui` adds one failure of its own: OFL-1.1, for the Inter font it embeds. Copying its style with the font would need OFL-1.1 allowed too.
  - It isn't offline by default: its style function installs an image loader that fetches any `http://` or `https://` image. Nothing is fetched unless such an image is shown, and the loader can be taken out after setup.
- **The toolkit alone isn't clean under today's `deny.toml`.** Three findings, each a decision for the maintainer rather than a defect:
  - wgpu is banned outside `silico-render` (Silico's own rule, "until M5's viewport");
  - egui's default fonts are OFL-1.1 and Ubuntu-font-1.0;
  - three unmaintained-crate advisories (RUSTSEC-2026-0370, 0371, and 0372) come in through `eframe`'s Linux accessibility bridge. The gate doesn't check advisories today (`deny.toml:52-53`).
- **Recommendation for decision 11.** Its condition reads "`re_ui` itself as a dependency only if the study finds its tree clean under `cargo-deny` and offline". Neither leg fails by much:
  - its one `cargo-deny` failure of its own, OFL-1.1, is needed by the copy too;
  - it is offline once its HTTP loader is taken out after setup.
  - So the condition is met only if the maintainer counts the loader's removal as offline.
  - The recommendation is still `re_ui`'s style constants, copied with attribution, and not `re_ui`: the app would otherwise link Rerun's data model and a gRPC and HTTP stack it never uses, carry a loader to remove, and follow `re_ui`'s widget API as it changes. This is a judgement, not a failed condition (section 5).
- **`egui_kittest` drives a UI through its accessibility tree.** A scratch test with egui 0.36.2, `egui_tiles` 0.17.1, and `egui_kittest` 0.36.2 did each of these, without pixels or a GPU. Without its `wgpu` feature nothing is drawn, so the viewport's paint callback and picking never run (section 4):
  - found a button by its role and label, clicked it, and saw the state change;
  - read a label's text;
  - clicked a tab;
  - scoped a query to one pane.
  - `egui_tiles` gives its panes no accessible name, so the app labels each pane itself (one call).

## 1. The versions `re_ui` 0.38.1 pins

From `re_ui-0.38.1/Cargo.toml`:

| Crate | Version | How `re_ui` takes it |
|---|---|---|
| `egui` | 0.36.2 | features `callstack`, `color-hex`, `rayon` |
| `eframe` | 0.36.2 | no default features; `accesskit`, `default_fonts`, `inspection`, `links`, `wayland`, `x11`, `wgpu` |
| `egui_extras` | 0.36.2 | features `http`, `image`, `serde`, `svg` |
| `egui_kittest` | 0.36.2 | optional (feature `testing`), and a development dependency; features `wgpu`, `snapshot`, `eframe` |
| `egui_tiles` | 0.17.1 | development dependency only |
| `egui_commonmark` | 0.25.0 | no default features; `pulldown_cmark` |
| `re_analytics` | 0.38.1 | optional (feature `analytics`, off by default) |
| `re_entity_db`, `re_log_types`, `re_uri`, `re_log`, `re_format`, `re_error`, `re_tracing`, and other `re_*` | 0.38.1 | always |

More on the versions:

- **egui and `egui_tiles`.** `egui_tiles` 0.17.1 asks for egui 0.36.0 (`egui_tiles-0.17.1/Cargo.toml`), so it resolves to 0.36.2 with the rest.
- **One wgpu.**
  - `egui-wgpu` 0.36.2 asks for wgpu 30.0 (`egui-wgpu-0.36.2/Cargo.toml`), and `re_renderer` 0.38.1 asks for wgpu 30.0 (`re_renderer-0.38.1/Cargo.toml`). All the trees here resolved wgpu 30.0.1.
  - `re_renderer` already enables wgpu's `gles`, `webgl`, and `webgpu` beside Vulkan and Metal.
  - `eframe`'s `wgpu` feature, which `re_ui` turns on, adds `egui-wgpu`'s and wgpu's default features. That brings `dx12` and `parking_lot` among them (`eframe-0.36.2/Cargo.toml.orig:94`; wgpu 30.0.1's `default`).
  - `eframe`'s `wgpu_no_default_features` is the same backend without those defaults. Silico's workspace then chooses wgpu's features, as it does today (`eframe-0.36.2/Cargo.toml.orig:96-103`).
- **Newer versions exist.** egui 0.37.0, `egui_tiles` 0.18.0, and `re_ui` and `re_renderer` 0.40.0 were on crates.io on 2026-10-27. The M5 plan pins the Rerun crates together and moves them with `re_renderer` (open decision 11), so 0.38.1's set is the one studied.

## 2. `re_ui`'s dependency tree under `cargo-deny`

Three trees were resolved against crates.io and checked with Silico's `deny.toml`:

- **re_ui:** `re_ui` 0.38.1 alone, default features.
- **toolkit:** `eframe` 0.36.2 with the features `re_ui` turns on that matter here (`accesskit`, `default_fonts`, `wayland`, `x11`, `wgpu`), `egui_tiles` 0.17.1, and `egui_kittest` 0.36.2 with `re_ui`'s features.
- **lean:** `eframe` with `wgpu_no_default_features` in place of `wgpu`; Silico's own wgpu features (Vulkan, Metal, WGSL, `std`); `egui_tiles`; and `egui_kittest` with no features, which accessibility tests don't need (section 4).

| | re_ui | toolkit | lean |
|---|---|---|---|
| Crates in the lock file | 657 | 390 | 373 |
| sources | ok | ok | ok |
| bans | wgpu banned | wgpu banned | wgpu banned |
| licences | OFL-1.1 (`re_ui`); OFL-1.1 and Ubuntu-font-1.0 (`epaint_default_fonts`) | OFL-1.1 and Ubuntu-font-1.0 (`epaint_default_fonts`) | the same |
| advisories | RUSTSEC-2026-0370 (`zvariant`), RUSTSEC-2026-0371 (`zbus_names`), RUSTSEC-2026-0372 (`zvariant_derive`): unmaintained | the same | the same |

What each finding is:

- **wgpu banned.** Silico's own rule: `deny.toml:23` allows wgpu only through `silico-render` and `re_renderer`, "until M5's viewport". Any app crate needs that rule changed (`eframe` and `egui-wgpu` are its direct parents in every tree). It isn't a finding about the crates.
- **OFL-1.1 on `re_ui`.** Its licence is `(MIT OR Apache-2.0) AND OFL-1.1`, for the Inter font it embeds (`re_ui-0.38.1/data/Inter-Medium.otf`, `OFL.txt`).
- **OFL-1.1 and Ubuntu-font-1.0 on `epaint_default_fonts`.** egui's default fonts (`eframe`'s `default_fonts`), whose licence is `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`. Without `default_fonts`, the app ships its own font, which is likely OFL-1.1 too, as Inter is.
  - Whether Silico's `deny.toml` allows OFL-1.1 is the decision, in either tree. It is a font licence, OSI-approved and FSF-free by `cargo-deny`'s own report; whether it suits Silico's GPL is the maintainer's call.
- **RUSTSEC-2026-0370, 0371, and 0372.** `zvariant`, `zbus_names`, and `zvariant_derive` are marked unmaintained ("No safe upgrade is available"; announcement `github.com/z-galaxy/zbus/issues/2043`).
  - They come only through `eframe`'s `accesskit` feature on Linux: `eframe` → `egui-winit` → `accesskit_winit` → `accesskit_unix` → `atspi` and `zbus`, the bridge to Linux screen readers.
  - `egui_kittest` doesn't need them: its own tree, with egui and `egui_tiles`, is 87 crates with no `zbus` (section 4).
  - So the choice is between the platform's screen-reader support on Linux, with these advisories ignored by name in `deny.toml`, and none.
  - The gate runs `cargo deny check bans licenses sources`, not advisories, which need the network (`deny.toml:1, 52-53`). So these don't fail the gate today; they will when advisories are checked.

What `re_ui` adds: 279 crates that the toolkit tree doesn't have. Every one of the groups below is already in Silico's own lock file through `silico-view` → `rerun` (for example tonic through `re_auth` → `re_redap_client` → `rerun`, ureq through `ehttp` → `re_redap_client`), where they pass the gate's checks. Grouped by what brings them into `re_ui`'s tree (`cargo tree -i`):

- **Rerun's data model:** `re_entity_db`, `re_log_types`, `re_chunk`, `re_chunk_store`, `re_query`, `re_sdk_types`, `re_sorbet`, `re_protos`, and Arrow (`arrow` and its eleven sub-crates). `re_ui` uses them for:
  - entity paths in its syntax highlighting (`re_ui-0.38.1/src/syntax_highlighting.rs:3-7`);
  - time types in its time widgets (`time_drag_value.rs`, `relative_time_range.rs`, `time.rs`);
  - a recording ID and a server origin in its command types (`src/command/environment.rs:7-10`, `src/command/table_command.rs:20-23`).
- **A gRPC and HTTP stack:** tonic (features `channel`, `server`, `router`, `transport`), hyper (`client`, `server`, `http1`, `http2`), axum, h2, tower, and `opentelemetry`. They come through `re_protos` and `re_grpc_headers`, which `re_entity_db` and `re_log_types` pull in.
- **An HTTP client with TLS:** ureq (`rustls`, `gzip`), rustls, ring, `webpki-roots`. They come through `ehttp`, which `egui_extras`'s `http` feature pulls in, which `re_ui` turns on.
- **Opening links:** `webbrowser`, through `egui-winit`'s `links` feature, which `re_ui` turns on in `eframe`.
- **File watching:** `notify`, a dependency on every native target, used only by `re_ui`'s hot reloading of its design tokens, which is compiled only for a debug build inside Rerun's own workspace (`re_ui-0.38.1/build.rs`: `DeveloperInWorkspace && !is_release`).
- **Others:** profiling (`puffin`), `regex`, `chrono`, `jiff`, `resvg` and `usvg` (SVG), the ICU crates, `rand`, `crossterm`, `cargo_metadata` (Rerun's build info), and more.

## 3. Does anything in it reach the network?

`re_ui`'s source, searched for `ehttp`, `http://`, `https://`, `install_image_loaders`, `re_analytics`, `open_url`, and `notify`, has four paths:

1. **Images by URL: yes.**
   - `re_ui::apply_style_and_install_loaders` calls `egui_extras::install_image_loaders` (`re_ui-0.38.1/src/lib.rs:238-242`).
   - With `egui_extras`'s `http` feature on (as `re_ui` turns it on), that installs `EhttpLoader` (`egui_extras-0.36.2/src/loaders.rs:65-71`).
   - The loader fetches any image whose URI starts with `http://` or `https://` (`egui_extras-0.36.2/src/loaders/http_loader.rs:65-76`), through ureq.
   - So after `re_ui`'s setup, any `egui::Image` given such a URI goes to the network: one from Silico's own code, or from rendered Markdown such as a help text.
   - The loader isn't installed unless that function is called. But it is the function that applies `re_ui`'s style.
   - It can be taken out afterwards: egui's `Loaders` keeps its byte loaders in a public `bytes: Mutex<Vec<...>>` (`egui-0.36.2/src/load.rs:613-618`), reached through `ctx.loaders()` (`context.rs:3929`).
2. **Links: on a click, through the system's browser.** `ui.open_url` (`src/ui_ext.rs:992-994`, `src/help.rs:160`, `src/link_button.rs:82`, `src/notifications.rs:567`) goes through `eframe`'s `links` feature to `webbrowser`, which asks the operating system to open the URL. The app makes no request itself.
3. **Analytics: no, by default.** The one call, `re_analytics::record` when a help button is first clicked (`src/ui_ext.rs:1149-1150`), is behind `#[cfg(feature = "analytics")]`. `analytics` isn't a default feature (`re_ui-0.38.1/Cargo.toml`: `default = []`).
4. **The gRPC and HTTP stack: compiled in, with no call from `re_ui` found.** `re_ui`'s source uses `re_uri` and `re_log_types` only as types (`src/command/environment.rs`, `src/command/table_command.rs`). Nothing in it starts a client or a server. The code is linked into the app all the same.

The lean tree has none of these crates: no ureq, `ehttp`, rustls, `webbrowser`, tonic, or hyper. Tokio is there only for WebAssembly targets (`winit` → `wasm-bindgen-futures`).

## 4. `egui_kittest` and the accessibility tree

**The API.**

- `egui_kittest` 0.36.2's `Harness` runs an egui UI headless, without a renderer unless one is asked for:
  - `new_ui`, or `new_ui_state` for a closure with a state;
  - `new_eframe` for an `eframe` app, with its `eframe` feature (`egui_kittest-0.36.2/src/lib.rs:235-250`).
- It implements `kittest` 0.4.0's `Queryable`: `get_by_label`, `get_by_role`, `get_by_role_and_label`, `get_by_label_contains`, and `get_by_value`, with `query_*` forms returning `Option` and `get_all_*` forms (`kittest-0.4.0/src/query.rs:146-240`). Each node is `Queryable` too, so a query can be scoped to a node's descendants (`kittest-0.4.0/src/node.rs:59`).
- `get_*` panics when nothing matches, or when more than one node does. Inside a test that's the assertion.
- **Text.** For a node of role `Label`, the label query matches its accessible *value*, where AccessKit keeps a label's text (`kittest-0.4.0/src/filter.rs:145-152`). A label's text is read with `node.value()` (`egui_kittest-0.36.2/src/node.rs:143-145`).
- **Actions.**
  - `node.click()` moves the pointer to the node's centre and clicks there (`node.rs:56-100`).
  - `node.click_accesskit()` sends AccessKit's `Click` action, which also reaches a widget that isn't on screen (`node.rs:103-115`).
  - `type_text`, `focus`, the scrolls, and key presses are there too.
- **Stepping.** `harness.run()` steps frames until the UI settles. `harness.state()` reads the app's state.
- **No GPU needed, and none used.** The `wgpu` and `snapshot` features are for rendering and comparing images (`egui_kittest-0.36.2/src/renderer.rs:16-21`). Neither is needed to drive a UI through its tree. But without them nothing is drawn, so a paint callback, the viewport's `re_renderer` callback among them, never runs: no GPU picking, so no click in the viewport selects anything.
  - Tests that select by clicking in the viewport (M5.7's, and M5.12's scripted session) need the `wgpu` feature and an adapter on CI (lavapipe). Or they select by name, through the browser or the command API.
  - Driving the `eframe::App` itself, with `Harness::new_eframe`, needs the `eframe` feature (`lib.rs:240-247`).
- **A UI that keeps repainting.** `harness.run()` steps until the UI settles and panics past its step limit. Its message says to "call Harness::step or Harness::run_steps instead" when the UI keeps repainting (`lib.rs:44-57, 356-362`). An app that polls a `Running` command every frame keeps repainting while one runs, so its tests step frames with `step` or `run_steps`.

**What a scratch test did** (egui 0.36.2, `egui_tiles` 0.17.1, `egui_kittest` 0.36.2 with no features, `cargo test`; both tests passed):

- An `egui_tiles` tree of two panes: a *Browser* pane with a label `Pad{n}`, and a *Toolbar* pane with a button `Pad` that adds one to `n`.
- `get_by_label("Pad0")` found the label.
- `get_by_role_and_label(Role::Button, "Pad").click()` and a `run()` made `n` 1. A query of every `Role::Label` node read back `value = "Pad1"`.
- `click_accesskit()` on the same button made `n` 2, without a pointer.
- With the panes as tabs, and before the panes were labelled (the first version of the test), the tree read:
  - `Window` → containers → `Button "Browser"`, `Button "Toolbar"`;
  - the active pane, an unlabelled `GenericContainer`, holding `Label` with value `"Pad0"`.
  - So `egui_tiles` 0.17.1's tabs are buttons (role `Button`, not `Tab`), and its panes have no accessible name.
- Labelling each pane in the pane's UI made the panes queryable by name:

  ```rust
  ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
      node.set_role(Role::Pane);
      node.set_label(name);
  });
  ```

  The call is `egui-0.36.2/src/context.rs:3684`. Then `get_by_role_and_label(Role::Pane, "Toolbar").get_by_role_and_label(Role::Button, "Pad").click()` clicked the button inside that pane. `get_by_role_and_label(Role::Pane, "Browser").get_by_role(Role::Label).value()` read `"Pad1"` from the Browser pane alone.

**For the app's tests (M5.4 on).** What the plan's tests need is all there:

- open a document and find its features on the timeline by name;
- find a failed feature's mark and its text;
- click Save.

Two conditions:

- Every panel the tests read is labelled, as above.
- Every control has a label or a role a test can name. Icon-only buttons need an accessible label, which also serves screen readers.

The viewport is a paint callback, one node with nothing inside it. It is tested through its ID buffer against projections by hand, as M4.6's renders are (open decision 7).

## 5. `re_ui`, or its style constants

Decision 11 recommends taking `re_ui` itself only if its tree is clean under `cargo-deny` and offline.

**What sections 2 and 3 found against that condition:**

- **Clean under `cargo-deny`: nearly.** `re_ui` adds one failure of its own, OFL-1.1 for Inter, which copying its style with the font needs allowed as well. Its gRPC and HTTP stack and its HTTP client are already in Silico's lock file through `silico-view` and pass the gate's checks. Everything else it fails, the toolkit alone fails too.
- **Offline: not by default.** Its style function installs an HTTP image loader. The loader fetches only an `http(s)` image that is shown, and can be taken out after setup (section 3).
- **What it costs beyond the condition.**
  - The app would link Rerun's data model, Arrow, and a gRPC and HTTP server and client it never calls, in the binary Silico ships.
  - It would carry a loader to remove after every style setup.
  - It would follow `re_ui`'s widget API, which moves with Rerun's releases.
  - `eframe`'s `wgpu` feature, which `re_ui` turns on, adds wgpu's `dx12` and `parking_lot`; `re_renderer` already sets the rest.
- **Recommendation:** its style constants, copied with attribution, and not `re_ui`. The condition is close enough to met that this is a judgement on those costs, the maintainer's to weigh.

**What the constants are.** `re_ui`'s look is data plus one function:

- **The data.** Design tokens in RON: `data/dark_theme.ron` (601 lines), `data/light_theme.ron` (590), and `data/color_table.ron` (940). These hold sizes, corner radii, margins, and named colours, many as aliases into the colour table, such as `"top_bar_color": "{Gray.100}"`.
- **The function.** `DesignTokens::load` reads them into a struct (`src/design_tokens.rs:251-560`), and `DesignTokens::apply` maps them onto `egui::Style` (`src/design_tokens.rs:794`). Accessors give the panel margins, the bar heights, and the list item height (`src/design_tokens.rs:1023-1150`).
- **Fonts and icons.** The Inter font (`data/Inter-Medium.otf`, OFL-1.1) and 122 icons (`data/icons/`).

**Copying them means:**

- the RON files, or the values Silico uses from them;
- a port of `apply` for the parts of the style Silico sets;
- `re_ui`'s licence notice: MIT or Apache-2.0, from Rerun's `LICENSE-MIT` and `LICENSE-APACHE`;
- the font under its own OFL-1.1, with its notice;
- the icons only as needed. They are Rerun's own art under the crate's licence, but Fusion's icons are what the app's layout follows (docs/ux/fusion.md), so few may be wanted.

**What is lost.** `re_ui`'s widgets: list items, the command palette, modals, notifications, the tab bar, and alerts (`src/list_item/`, `src/command_palette.rs`, `src/modal.rs`, `src/notifications.rs`). The app would build what it needs on egui directly, or copy individual widgets with the same attribution. The plan's M5.4 asks only for "an `eframe` window styled as `re_ui` styles Rerun".

## For M5.0 and the maintainer

1. **Decision 11.** Style constants rather than `re_ui`. That is a judgement on what `re_ui` would link into the app, since its condition is nearly met (section 5). `eframe` with `wgpu_no_default_features`, which keeps `dx12` and `parking_lot` out of wgpu; `re_renderer` already sets the rest of wgpu's backends (`re_renderer-0.38.1/Cargo.toml`).
2. **`deny.toml`, three changes any app crate needs, each the maintainer's:**
   - wgpu allowed through the app crate (or through `eframe` and `egui-wgpu` as wrappers);
   - OFL-1.1 allowed for fonts, and Ubuntu-font-1.0 too if egui's default fonts are kept;
   - RUSTSEC-2026-0370, 0371, and 0372 ignored by name with a reason (once the gate checks advisories), or `eframe`'s `accesskit` feature left off on Linux at the cost of screen readers there.
3. **Accessibility tests (decision 7).** `egui_kittest` through the tree, with labelled panes and labelled controls; pane labels are one call each in the pane's UI (section 4).
   - Panels and controls need no features.
   - Selecting by a click in the viewport needs the `wgpu` feature and lavapipe on CI, or selection by name instead.
   - Frames are stepped with `step` or `run_steps` while a command runs.

## Method

Scratch crates outside the repository. Their manifests and the test are in a comment on the PR that added this note (#223); the crates themselves are removed once it merges. Each was resolved with `cargo generate-lockfile` against crates.io on 2026-10-27, then checked with `cargo-deny check` (0.20.2) using a copy of Silico's `deny.toml`:

- **`re_ui` probe** (the re_ui column of section 2): depends on `re_ui = "=0.38.1"` alone.
- **`bare-probe`:** `eframe = "=0.36.2"` (no default features; `accesskit`, `default_fonts`, `wayland`, `x11`, `wgpu`), `egui_tiles = "=0.17.1"`, and the development dependency `egui_kittest = "=0.36.2"` with `wgpu`, `snapshot`, and `eframe`. This is the toolkit column of section 2.
- **`lean-probe`:** the same, with `wgpu_no_default_features` in place of `wgpu`; wgpu 30 with Vulkan, Vulkan portability, Metal, WGSL, and `std`; and `egui_kittest` with no features. It leaves out `re_renderer` and `silico-render`, so 373 isn't the app's crate count, only the toolkit's.
- **`kittest-probe`:** egui 0.36.2 and `egui_tiles` 0.17.1, with `egui_kittest` 0.36.2 (no features) as a development dependency. Two tests, built under the build lock with three jobs and run once each. The unlabelled tree in section 4 is from its first version, before the pane-labelling call was added.

The crate counts are the lock files' `[[package]]` entries, which cover every target. Paths into a crate are into its published source in Cargo's registry.
