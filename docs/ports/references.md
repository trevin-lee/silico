# References by name

How a feature that names elements of another (a fillet's edges, a chamfer's) finds them again after an upstream edit (M2.8). The names are M2.7's (docs/ports/naming.md); the decisions are the maintainer's, N4 to N6 in docs/design/m2.7-naming.md. This note covers what FreeCAD does, where Silico departs from it, the bug fixes FreeCAD's history records, and the tests ported.

FreeCAD's source here is `~/Git/FreeCAD` at `876271d660`.

## What FreeCAD does

### Storage

A reference is a `PropertyLinkSub`: the linked object and, for each sub-element, the text the user set and a shadow pair (`ShadowSub`): the mapped name (`;<mapped>.Edge5`) and the index name (`Edge5`, or `?Edge5` once the element is missing, `src/App/ElementNamingUtils.h:52-57`). The file keeps both and the `?` marker (`src/App/PropertyLinks.cpp:1905-2014`).

### Resolution (`_updateElementReference`, `PropertyLinks.cpp:449-604`)

It runs when the linked feature's shape changes (`GeoFeature::updateElementReference`, `src/App/GeoFeature.cpp:252-284`), before the features downstream recompute, and never fails the recompute (an exception is logged, `PropertyLinks.cpp:294`).

1. The mapped name decides: it's looked up in the new map (`ElementMap::find`, `src/App/ElementMap.cpp:823-877`, one element per name by construction). Found, the index is updated silently (`Face8` becomes `Face9`; `testBaseFeatureAttachmentSupport` asserts it).
2. An element with no mapped name is followed by its number.
3. A name not found: the old element's sub-shape, cached before the change (`Part::Feature::onBeforeChange`, `src/Mod/Part/App/PartFeature.cpp:1440-1523`), is searched for in the new shape: by geometry, then by vertices alone (`dbd2bade1e`), then, with a tolerance doubling up to 1e-3 of the element's size, for a unique match (`d45ff368a7`). The first two take the first of several matches (`names.front()`, `:519`) and log it at debug level (`837f6fa788` demoted the warning).
4. Nothing found: the index becomes `?Edge5`, the lost mapped name is kept (so the reference heals if a later edit brings the name back), and a warning: `<prop> missing element reference <obj> <name>` (`:549-553`). A reference already `?` is never searched by geometry again (`:505-506`).

Duplicates are settled when a map is built (`;D` and a random number in release builds, `ElementMap.cpp:756-779`), so a reference to one breaks on the next rebuild and falls to the geometric search.

### A fillet's or chamfer's edges

- PartDesign (`DressUp::getContinuousEdges`, `src/Mod/PartDesign/App/FeatureDressUp.cpp:255-320`): the first edge that doesn't resolve throws `Invalid edge link: <ref>` (`45c4aefaae`); the feature fails, naming that one edge only. An edge without two faces, or a tangent one, is skipped with a warning; a face's or a solid's edges are taken in its place; with every edge skipped, `Fillet not possible on selected shapes` (`FeatureFillet.cpp:95-98`) or `No edges specified` (`FeatureChamfer.cpp:143-145`).
- Part workbench (`Part::Fillet::execute`, `src/Mod/Part/App/FeatureFillet.cpp:71-114`, `68077de39b`): every missing edge is gathered, `Missing edge link: <ref>`, and the feature fails with all of them.
- The GUI's DressUp dialog replaces each `?` reference with a guess, the first element related by history (`guessNewLink`, `PartFeature.cpp:2015-2024`, `80045d0e88`), and recomputes; `19e382d5b0` fixed it dropping the good edges. Deleting a feature relinks a dressup to the new base only where each edge has exactly one geometric match (`Feature::relinkToMatchingSubelements`, `src/Mod/PartDesign/App/Feature.cpp:221-264`, `26c895c30d`).

## What Silico does

ARCHITECTURE.md forbids a reference silently re-pointing, so three of FreeCAD's paths go: following an element by its number, taking the first of several geometric matches, and the dialog's guess.

- **Stored by name only** (N5): a fillet's or chamfer's `edges` are Silico names, written as M2.7 writes them, with no number beside them and no `?` marker. A rebuild never rewrites a reference, so, as FreeCAD's kept mapped name does, a reference broken by one edit resolves again when a later edit brings its element back.
- **Resolved exactly**: a name is parsed (`naming::Name::parse`, the grammar M2.7 writes) and looked up in the base's map by identity (which leaves out OCCT's raw positions and a bitten element's last operation, as M2.7's identity does, and holds every feature's type as the name writes it, and every source's side: `k7f2qx3m.box.Edge1` isn't a cylinder's `Edge1`, nor `c.cut.Edge(…)` a common's, nor a cut's `M1/2` piece of its base its `I1/2` piece of its tool: N8, N9). It resolves to an element only if that element is an edge and its name isn't unstable (a duplicate `~n`, or an element named by its number: N3). An edge no history names is named by the faces it bounds, not by its place among a face's edges (Trevin's, docs/ports/naming.md), so an edit that takes it away breaks the reference; FreeCAD's upper pass gives its name to another edge.
- **Any broken reference fails the feature** (N6): every broken edge is reported at once, each a `reference_broken` error (N4) naming the feature, the parameter (`edges.2`), and the reference as written, with candidates; the features using it are blocked (M2.4). FreeCAD's Part workbench does this; PartDesign stops at the first.
- **Candidates, never applied** (N4): the elements of the base's map whose names share the broken name's sources (a hole moved: the edges still generated from the same faces), so an agent sees what the reference most likely meant. They're listed by how many of the broken name's sources each shares, most first, then those of the broken name's own feature, then by name; if none shares any, an element by its number that names nothing (`box.Face7`) offers its feature's elements numbered nearest it (`box.Face6`), by name; at most five. A measure's broken names (M4.5) are offered the same way, from every live solid. FreeCAD's geometric search (identical geometry in the last good build) needs the kernel to compare elements, and comes later.
- **No skipping**: an edge that can't be filleted is the kernel's refusal (M2.0's `EdgesFailed`), not a warning and a smaller fillet.
- **The kernel's refusals name the document's edges**: an edge OCCT can't fillet, or refuses, is named in the error by its name as the document writes it (its elements) and its place in `edges` (its words, and the parameter when it's the only one), never by its number in the base, which no document holds. FreeCAD's PartDesign reports a failed fillet without naming the edge.
- **An edge two names find is filleted once**, as PartDesign's `shapeSet` takes it (`FeatureDressUp.cpp:263`): each name still finds that edge, so nothing re-points. One name written twice is refused by the gate.
- **Bounded**: a name is read only as deep and as wide as a map's names are (256 levels, a `~n` a level; 4,096 names written out); a feature lists its first hundred broken references, each with candidates, and counts them all (`FeatureOutcome::Broken`'s total, and so a check's), as the gate lists its problems and counts them.
- **Not yet**: references to faces. FreeCAD's PartDesign takes a face for its edges (`FeatureDressUp.cpp:302-306`; `TestFillet.py testFilletCubeToSphere`, whose Silico port names the 12 edges); Silico's fillets and chamfers take edges only, until a face's edges are a reference of their own (a face's edges change with an edit, so which edges a face reference means is a decision of its own).

## Bug fixes in FreeCAD's history

| Commit | Fix | In Silico |
|---|---|---|
| `8fdef156f0` | `resolveElement` compared the link object, not the geometry, with its filter: references through links never updated | No links yet |
| `45c4aefaae` | A PartDesign dressup's missing edge throws `Invalid edge link` instead of being dropped | A broken reference fails the feature |
| `68077de39b` | Part fillet and chamfer fail, listing every missing edge | The same (N6) |
| `80045d0e88`, `19e382d5b0` | The dialog's guess for missing edges; it dropped the good edges | No guesses: candidates only |
| `837f6fa788` | The geometric re-point logged at debug level, not as a warning | No geometric re-point |
| `dbd2bade1e`, `d45ff368a7` | Vertex-only and widening-tolerance geometric searches | No geometric re-point; candidates later |
| `26c895c30d` | Relink to a new base only on a unique geometric match | Rerouted by remove and move (A9, docs/ports/chain.md); a name that passed through the removed feature is found on the new base; refused where the removed feature made the element |
| `0a3677b9eb` | Element maps with more than two children silently corrupted on load | Maps aren't stored; names are rebuilt from history |
| `e9ea3d0d25` | A map version change forces every reference through the geometric search | A change to the naming grammar is the format's change, made with the maintainer |

## Tests

| FreeCAD test | Status |
|---|---|
| `TestTopologicalNamingProblem.py` `testPartDesignTNPChamfer`, `testPartDesignTNPFillet` (a dressup's edges changed; what's attached to its face survives) | Ported as Silico's cases (silico-kernel `tests/references.rs`): a chamfer, and a fillet, of a box's Edges 1, 5, and 7, then Edge1 dropped from it; a fillet downstream, of an edge of Edge7's dressup's face named by the faces it bounds, still resolves: to the same chamfer edge by what its fillet takes, and to the same fillet arc by where it is |
| `testBaseFeatureAttachmentSupport` (a fuse's box made wider; the reference follows `Face8` to `Face9`) | Ported: a fillet of an edge of the widened box's face resolves to the edge that moved, and the name in the document is unchanged |
| `testPadChange_UpToFirst_to_Dimension`, the sketch tests | M3, with sketches and pads |
| `TestFillet.py` relink tests | Not ported: no relinking |
| None in FreeCAD | Silico's own (Trevin's, 2026-10-17): a primitive made another type under its ID, for every pair of types, and a merge of that with a new fillet on other lines, breaks the fillet; a chamfer's edge named by its faces breaks when a second chamfer takes it away (Edge8, Edge12), offered the edge now there, while the other still takes the same; a fillet's own edges and vertices keep their names through a hole elsewhere |
| None in FreeCAD | Silico's own: a reference to an edge an edit removed fails, `reference_broken`, with candidates; a reference to a `~n` or a numbered name fails; a reference to a face where an edge is wanted fails; every broken edge of a feature is reported; after random edits, a reference that resolves names an element with the same derivation (never re-points); a reference broken by one edit and restored by the next resolves again |
