# The M5 UI prototype

This is the approved reference for the app's look and layout (Trevin, 2026-11-01: "looks good, build it"). Every port PR cites it, and each is checked against it with a screenshot.

Open `index.html` in a browser. There's no build step and no network: three.js r149 and Inter Medium are vendored here. The **Prototype ▾** pill at the top right jumps between the states under review. `DESIGN.md` gives the design and its reasons, the tokens, how each part maps to egui, and the approval, which takes every open question as recommended.

The prototype stands in for the app. Its geometry, element names and rebuilds are illustrative. Where it goes past the approved M5.0 design, `DESIGN.md` says so under *Proposals beyond M5.0*. The two proposals that need core changes (a per-feature change summary, and *Restore my version* across a reload) are designed separately before anyone builds them.

Licences: `vendor/three.min.js` is MIT (`vendor/LICENSE-three.txt`). `fonts/Inter-Medium.otf` is under the SIL Open Font License (`fonts/OFL.txt`), copied from re_ui 0.38.1. Nothing here is built into Silico.
