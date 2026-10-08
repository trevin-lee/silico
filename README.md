# Silico

A parametric CAD system: a Rust core, an egui/wgpu app, and OpenCascade as the geometry kernel. Agents and people are equal users (through MCP and the app), and models are plain, diffable text: a timeline of features, rebuilt bit for bit on any machine.

**Status:** M0 (foundations). OpenCascade runs in worker processes behind a structured, crash-proof protocol, built deterministically, and checked against FreeCAD. There's no app yet; `pixi run view` shows what the kernel builds.

## Try it

```bash
pixi run test   # builds OpenCascade (about 3 minutes the first time), then every test
pixi run view   # a box and two fillets in the Rerun viewer, edges labelled
pixi run gate   # every check, on the committed code
```

Requires [pixi](https://pixi.sh) and [rustup](https://rustup.rs). macOS (Apple silicon) and Linux (x86-64).

## Read more

- [docs/walkthrough/m0.md](docs/walkthrough/m0.md): how the pieces fit, and how to check them yourself.
- [ARCHITECTURE.md](ARCHITECTURE.md): the design and its invariants.
- [ROADMAP.md](ROADMAP.md): what comes next.
