# Topological naming

How every face, edge, and vertex of a feature's solid gets a name that says how it came about, so a reference to it can follow it through upstream edits (M2.7), and how a reference finds its element again (M2.8). The decisions are the maintainer's, in docs/design/m2.7-naming.md. This note covers what FreeCAD does, what Silico does instead, the bug fixes FreeCAD's history records, and the tests ported.

FreeCAD's source here is `~/Git/FreeCAD` at `876271d660`; names quoted as "observed" were printed by the oracle FreeCAD (1.1.3, OCCT 7.9.3).

## What FreeCAD does

### Names

A FreeCAD element name (`MappedName`, `src/App/MappedName.*`) is one flat string holding the element's derivation, each operation adding a segment:

```
Face6;:G2(Face1;K-2;:Ha91,F);CUT;:Hk7f:1d,E
```

"generated (`;:G`), second in OCCT's list (`2`), from the box's `Face6`, and from the cylinder's `Face1` (generated, second: `;K-2`), by a cut (`CUT`); the first source belonged to object `k7f`; the segment is `0x1d` characters long; an edge." The markers (`src/App/ElementNamingUtils.h:51-92`): `;:M` modified, `;:G` generated, `;:MG` both, `;:U<n>` the `n`th unnamed element of the element named before it, `;:L` named from the elements below it, `(a|b|...)` up to four more sources, `;K<n>` another source's signed position, `;:H<tag>[:<len>],<type>` the tag segment, `;D<n>` a duplicate. Op codes are three letters (`src/Mod/Part/App/TopoShapeOpCode.h`): `FUS`, `CUT`, `CMN`, `FLT`, `CHF`, `XTR`, `RVL`, and more. A move has none: `makeElementTransform` (`TopoShapeExpansion.cpp:3485-3530`) moves the shape and keeps its map, never asking OCCT for history.

A primitive has no map at all: its elements' names are their numbers (`Face3`), in OCCT's `TopExp::MapShapes` order.

In a document FreeCAD replaces nested names with numbers from a per-document string table (`StringHasher`, `#3:1;:U;XTR;:H12d7:7,E`), its tags are session object IDs (a new document starts them at a random offset, `src/App/Document.cpp:127-135`), and a reference stores the index name beside the mapped name (`sub="Vertex2" shadow=";#16;:U;XTR;:Hc1f:7,V.Vertex2"`).

### The algorithm (`makeShapeWithElementMap`, `TopoShapeExpansion.cpp:1420-2010`)

1. **Kept, by identity** (`mapSubElement`, `:912-1012`): an input element the result has as the same OCCT sub-shape keeps all its names, with the input's tag and no op code (`a4dd600ec6` took the op code off).
2. **Modified and generated** (`:1483-1659`): for source kinds vertex, edge, face, each input in order, each element: OCCT's `Modified` and `Generated` lists, each entry counted (its position) whether or not the result has it. An unnamed result element collects its sources under `NameKey` (`:1277-1322`): the source's kind (vertex < edge < face; a whole solid generated ranks after, "high level"), then the input's tag, then the source's primary name byte by byte. Generated is read after modified, so one source that did both to one element counts as generated.
3. **Naming loop** (`:1664-1816`), elements in `IndexedName` order (`Edge` < `Face` < `Vertex`): the first source, its relation and position, up to four more sources with signed positions (`;:MG` when they disagree), the op code, the first source's tag.
4. **Upper pass** (`:1828-1904`): for (face, edge) then (edge, vertex), each named upper element's unnamed lower elements, counted in `TopExp::MapShapes(upper, lower)` order; each lower element takes the upper whose name sorts first by `ElementNameComparator` (`src/App/MappedElement.cpp:240-259`). Silico names it from every named upper instead (below).
5. **Lower pass** (`:1908-2003`): each unnamed edge from its vertices, then each unnamed face from its outer wire's edges (`BRepTools::OuterWire`, holes left out), once all are named: the first by the comparator, then up to four more.
6. **A second round** (`:2004-2007`) only if something is still unnamed and high-level entries remain (an extrusion's top face, named from the whole solid its profile generated, with `;:G0` for the first face parallel to the profile, `checkForParallelOrCoplanar`, `:1363-1417`).

### Resolving a reference (`src/App/PropertyLinks.cpp`, `GeoFeature.cpp`)

After a recompute FreeCAD looks each reference up by its mapped name and rewrites the index. If the name is lost, it re-points the reference to whatever element is where the old one was (exactly, then within tolerance, then within 1e-3 of the element's size, `d45ff368a7`; demoted from warning to log in `837f6fa788`), or falls back to a coplanar face or matching edge (`5266a3b26d`, #16803); otherwise it marks the reference `?Edge3` and the feature fails ("Invalid edge link"), and opening the feature's dialog replaces the lost reference with a guess (`80045d0e88`). FreeCAD has no test that a reference fails loudly.

## Defects in FreeCAD that Silico doesn't copy

- **Random duplicate names.** `renameDuplicateElement` (`ElementMap.cpp:756-780`) appends `;D` and a random number in release builds (`std::random_device`), the attempt number only in debug builds; FreeCAD's own test expects `;D1` (`tests/src/Mod/Part/App/TopoShapeExpansion.cpp:1811`). Observed: one ruled surface gave `Edge1;D16ad` in one run and `Edge1;D1b4a` in the next.
- **Stale text in names from below.** With one lower element, the lower pass writes `;:L` without clearing the stream (`:1967-1969`; the other branch clears it, `:1972`), so the previous element's postfix leaks in. Observed on a full revolve: `Face3 = Vertex3;:G;RVL;:H...;:L;RVL;:H...;:L;RVL;:H...` (Face1's postfix repeated inside it). Silico names such a face from its edge alone.
- **Abbreviated names resolve.** `IndexedName::set` matches types by prefix (`IndexedName.cpp:105-109`): `F3` and even `3` name `Face3` (observed). `cc9387aa3f` claimed to fix it and didn't. Silico's names parse exactly (M2.8).
- **Process-dependent orders.** `MappedName::compare` compares signed `char`s (`MappedName.h:798-806`), and some listings iterate a `QHash`. Silico's orders are total and on bytes.
- **Misaligned parallel faces.** The `;:G0`/`;:G00` position is counted within one expanded shape but compared with a counter across the source's whole `Generated` list (`:1576-1652`). Silico takes an extrusion's ends from the kernel's history (`History::ends`) when extrusions arrive (M3).
- **Silent re-pointing** (above). ARCHITECTURE.md forbids it; Silico's broken references fail, listing candidates (M2.8).

## Bug fixes from FreeCAD's history

| Commit | What it fixed | In Silico |
|---|---|---|
| `a4dd600ec6` (2024-04) | Kept elements carried the op code. | Kept names are kept exactly (N2). |
| `d43fe277a7` → `8fff92618c` (2024-01, 2024-05) | A refactor passed `parallelFace` by value, so an extrusion's top face lost `;:G0`; fillets on a pad's top broke. | Extrusions (M3) name their ends from `History::ends`. |
| `c999f887eb` → `140c9febc4` (2024-01, 2025-09, #22889) | A lint turned `ElementNameComparator` into a non-strict order; `std::map`s keyed by it were corrupted and old models crashed. Rewritten, with map-order tests. | Ported as rewritten (`naming::freecad::compare`), with its tests. |
| `a88e7baff5` (2024-04) | Dehashing returned the hash itself, so history tracing stopped at the first hashed level for ten months. | No string table: names are written out in full. |
| `9f2d212b12`, `88b5cde45f`, `6df651b5c8` (2025) | Three changes to when child maps are stored compactly, each renaming elements in some models; `e9ea3d0d25` then forced old files to rebuild their maps. | No child maps: a move keeps its input's map; one solid per step. |
| `0a3677b9eb` (2025-04) | An operator-precedence bug in a restore cap rejected files with more than two element maps, silently corrupting projects for seven weeks. | Maps aren't stored; they're rebuilt from history (and the cache). |
| `0e7aa5f7bc`, `99b753b006` (2024) | Segfaults on a null element map. | Maps are values, never null. |
| `8bac0ce895` (2026, #30444) | An out-of-range `topoShapes[index-1]` crash from naming sub-shapes through the parent. | Every lookup is checked (`get`), and histories are validated first. |
| `f9b65e563d` | Removed realthunder's fallback to the index name as "too risky". | A reference never falls back to an index: an element nothing names is named by its number only as a last resort, and that name is unstable, refused as a reference (N3). |

## Silico

### Names (N1, N2)

`naming::Name` is a structured value: an `Element` (a feature's own element by number: a primitive's, which says its type, or the last resort), a `Derived` element (the operation, the result's kind, the sources each with relation (`M` modified, `G` generated, `I` kept inside the other operand: N9), OCCT's position (none for an element kept whole), and, where needed, a place `n/m`, first by FreeCAD's key, up to five), an `Upper` element (from every named element above it, up to five, and, where needed, a place `n/m`), a `Lower` element (from the elements below, up to five), a `Duplicate` (`~n`), or a `Modified` element (a source modified one from one: its source's name, with the operation as data). Written in Silico's form, the maker feature's ID, its type (the document's word: `box`, …, `cut`, `fillet`, read from the name, never from the feature as it is now: N8), the kind, and how it came about:

| Silico | FreeCAD (without tags) |
|---|---|
| `k7f2qx3m.box.Face3` | `Face3` |
| `m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)` (the hole's wall, a piece of the tool) | `Face1;:M;CUT` |
| `cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge3)` (a void's rim: the tool kept whole inside the base) | `Edge3` |
| `cccccccc.cut.Edge(I1/2 aaaaaaaa.box.Edge9)` (a split edge's piece inside the other operand) | `Edge9;:M;CUT` |
| `pppppppp.box.Face6` (after a hole: bitten) | `Face6;:M;CUT` |
| `m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)` | `Face6;:G2(Face1;K-2);CUT` |
| `cccccccc.common.Edge(I aaaaaaaa.box.Edge9)` (a box's edge inside a common) | `Edge9;:M;CMN` |
| `m3tq8v0z.cut.Face7` (the last resort, unstable) | `Face7` |
| `c.cut.Edge(G1/2 b.sphere.Face1, G1/2 p.cylinder.Face1)` (a ball's through hole) | `Face1;:G(Face1;K-1);CUT` |
| `f.fillet.Edge(U f.fillet.Face(G e.box.Edge1), e.box.Face6)` (a fillet's arc on the top) | `Edge1;:G;FLT;:U2;FLT` (the first above, and its count) |
| `f.fillet.Face(L a, b, c)` | `a;:L(b|c);FLT` |
| `<name>~1` | `<name>;D1` |

An element kept whole of an input the operation bites (its outside kept: a cut's base, a fuse's operands, a fillet's or chamfer's input) keeps its name exactly, through any number of features (N2); kept whole of one it doesn't, it's a piece, `I` (N9, below): FreeCAD adds a tag level per feature. An element an operation modified from one element, as its sole result of it (nothing else of its kind modified or generated from it), is that element's name exactly (`pppppppp.box.Face6`, N7), however many features in a row did so, where the operation keeps what of it lies outside its other operand, a *bite* (`OpCode::bites`: a cut's base, a fuse's operands, a fillet's or chamfer's input): FreeCAD adds a level for each (`Face6;:M;CUT;:H..;:M;CUT;:H..`), and Silico's FreeCAD form writes one. It's safe because a solid is consumed once (M2.1's D2), so an element's lineage is a tree and one origin can't reach a shape twice; patterns and mirrors, which copy, will add a copy marker. The last operation isn't part of the name's identity, but is held, for FreeCAD's form and FreeCAD's orders.

Everything an operation keeps of an input inside its other operand (a cut's tool, a common's operands), kept whole, trimmed, or split, is a *piece*: the feature's own derived element, its source written `I` (N9): the hole's wall `m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)` (FreeCAD's `Face1;:M;CUT`), a void's rim `cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge3)` (FreeCAD's `Edge3`: a kept element has no OCCT position, and FreeCAD's form writes it as its source, as FreeCAD's `mapSubElement` does), a split edge's pieces `cccccccc.cut.Edge(I1/2 aaaaaaaa.box.Edge9)`, `I2/2`. `M` is only what a biting input's source became (a split's `M1/2`, a sole result OCCT lists second, a merge); a sole `M` at OCCT's first position collapses to its source (N7), and an `I` never does. The milestone review proved the re-pointings this closes: a cut made a common, and a cut's base and tool swapped, each took a reference to a box's Edge9 for another piece of it (MF1; N8 wrote the operation's word), the same for an edge split in two on both sides (R2F1), and for a tool's element kept whole, bare under any operation (R2B1). Which side an operation keeps is fixed by the booleans' definitions (a cut keeps its base's outside and its tool's inside, a common both insides, a fuse both outsides), so `bites` is a table, read in two places, both to pick the letter: a kept element (bare or `I`) and a modified one (`M` or `I`). A cut made a fuse keeps a base's element bitten alike, and the kernel gives the same element: nothing re-points, and none is reported. A piece keeps one name whole or trimmed: a void's seam is `cccccccc.cut.Edge(I bbbbbbbb.cylinder.Edge1)` with the pin inside the block and with it moved through the floor. N8 and N9 (Fable's, 2026-10-18, docs/design/m2.7-naming.md).

The residual (N9): a vertex OCCT generates from an edge of one input and a face of the other (`cccccccc.cut.Vertex(G aaaaaaaa.box.Edge9, G jjjjjjjj.box.Face2)`) lists its sources by kind before input, so under a cut's base and tool swapped its text is the same. It names the same point in space, FreeCAD names it alike, and nothing in M2 references a vertex; if one ever needs the solid's corner, not the point, a cut ordering its generated sources by input first is the fix, at a scheme bump. Recorded.

`naming::freecad::text` renders FreeCAD's form without tags, for comparing with FreeCAD's names and for FreeCAD's orders (which source comes first, which element above or below names an element). Silico orders inputs by their place in the operation (base, then tool), where FreeCAD orders them by tag (object IDs, in creation order): the same in FreeCAD's tests and in documents built in order.

### Positions only where names collide (a departure from FreeCAD, N7)

FreeCAD writes every source's position in OCCT's `Modified` and `Generated` lists (`;:G2`, `;K-2`). Silico doesn't: OCCT orders those lists for reasons of its own, and measured on the real kernel, a plate's last hole's rims swapped positions (`G2` and `G`) depending on whether an earlier hole had been cut, which renamed them. So names are built without positions, grouped by kind and name, and only within a group of m ≥ 2 is each source written `n/m`, the element's rank among the group by its position in that source's list (members at one position share a rank): `G1/2`, `G2/2`. Where members still coincide, they're numbered `~n` and unstable (N3). An `n/m` name is stable. The raw positions are kept in the name, outside its identity, so `naming::freecad::text` still writes FreeCAD's positions, and the ported fixtures still match FreeCAD's names exactly; a split's ranks are FreeCAD's positions in every fixture (the two cubes' fuse: `M1/2` is `;:M`, `M2/2` is `;:M2`).

Fable's census (2026-10-16), which the tests reproduce: names collide only where an element splits (a slot across a face or an edge, a groove across a U: `M1/2`, `M2/2`, `M3/3`) and where curved faces meet curved faces in loops (a ball's through hole: two rims and two seam points; a pin through a torus's tube: four loops `G1/4` to `G4/4`, the seam in three; a tube's wall). In each, positions are the only thing that tells the elements apart; FreeCAD tells them apart the same way.

The residual case: OCCT reordering look-alikes the same way in every list after an upstream edit, so two ranks swap and a reference silently follows the other element. It's FreeCAD's case too (its positions swap alike), and wasn't reproduced. It's recorded here, not guarded. Elements named from above are ranked by one list, their count in the first element above (the port review, round 2): there one list reordered is enough to swap two ranks. It needs two edges (or vertices) no history names bounding the same named faces; none that a fillet or chamfer takes was found in M2 (the seams and degenerate edges of a corner blend are, and can't be filleted). Recorded, not guarded.

### A primitive's elements say its type (Trevin's, 2026-10-17)

FreeCAD's primitive names are bare numbers (`Face3`), and so were Silico's (`k7f2qx3m.Face3`): a box made a cylinder under the same ID, by an agent or by a merge of two branches, kept every reference to its `Edge1`, which then named the cylinder's. Trevin decided a primitive's element says what made it, in the document's own type words: `k7f2qx3m.box.Face3`, `a91cw0de.cylinder.Face1`, `s0000001.sphere.Edge2`, `c0000001.cone.Face2`, `t0000001.torus.Edge1` (`naming::PrimitiveKind`). The type is part of the name's identity, so a primitive made another type breaks every reference to its elements, `reference_broken`, and never re-points. N8 (2026-10-18) gave an operation's elements the same: `m3tq8v0z.cut.Edge(…)`, and `m3tq8v0z.cut.Face1` the last resort's number, unstable; a cut made a common breaks a reference to its rim, with the common's rim the candidate. The parser reads the type as written, refusing a name without one and listing the ten words, and a primitive's word before `(` (a primitive names its elements by number); a word another than the feature's now reads, and finds nothing, `reference_broken`, saying what changed ("names an element a cut made, but cccccccc is a common now"). FreeCAD's form (`naming::freecad::text`) is unchanged: `Face3`. Tested on the fake for every pair of types and for a merge of a type change and a new fillet on other lines (silico-core `tests/references.rs`).

### An element no history names is named by every named element it bounds (Trevin's, 2026-10-17, a departure from FreeCAD)

FreeCAD's upper pass (step 4) names an unnamed edge from one face only, the face whose name sorts first, as that face's `n`th unnamed edge in OCCT's order (`;:U4`). The `n` is a position, so an edit that takes one of that face's edges away gives its number to another: the port review found it, and FreeCAD 1.1.3 does it too (a PartDesign box, a chamfer of `Edge7`, size 1; then of `Edge7` and `Edge8`, `Edge11`, or `Edge12`; each edge's midpoint printed):

| Chamfered | `Edge7;:G;CHF;:U;CHF` | `Edge7;:G;CHF;:U4;CHF` |
|---|---|---|
| `Edge7` | (9.5, 9.5, 0), the chamfer's bottom edge | (9.5, 9.5, 10), its top edge |
| `Edge7`, `Edge8` (or `Edge11`) | (9.5, 9.5, 0.5), the edge between the two chamfers | (9.5, 9.5, 10) |
| `Edge7`, `Edge12` | (9.5, 9.5, 0) | (9.5, 9.5, 9.5), the edge between the two chamfers |

A reference to the bottom edge silently moves to another edge, which ARCHITECTURE.md forbids. Trevin decided: an edge no history names is named by every named face it bounds, a vertex by every named edge it ends (up to five, in FreeCAD's order: `cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face5)`, the chamfer's bottom edge), and given a place `n/m` only among the elements named by the same set, ranked by FreeCAD's count, as N7 ranks collisions (`U1/2`, `U2/2`). An edit that takes the edge away breaks a reference to it; the edge between the two chamfers is `cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), cccccccc.Face(G aaaaaaaa.box.Edge8))`, offered as a candidate, never taken. A fillet's face's edges and ends, which OCCT gives no history, are each referenceable this way, and keep their names through an edit elsewhere that renumbers them. FreeCAD's form keeps FreeCAD's choice, the first face above and its count (`Edge7;:G;CHF;:U4;CHF`), so the ported fixtures still match FreeCAD's names exactly. Tested on the real kernel (silico-kernel `tests/references.rs`: the Edge8 and Edge12 cases, by the fillets' volumes worked out by hand, and the fillet's own elements through a hole) and in the TNP chamfer port, whose edge is now named by the chamfer's face and the box's `Face2`.

### A primitive's elements numbered as in its fullest form (Trevin's, 2026-10-17)

FreeCAD names a primitive's elements by OCCT's numbers. A cone brought to a point loses its top cap, and OCCT numbers the base cap Face2, the top cap's number: so names derived from it say the base where they said the top, and a reference to the top rim moves to the bottom rim. FreeCAD fails there only because its names keep OCCT's list positions, which Silico's don't (above); `/break-it` found the move. Trevin decided: each primitive's elements are numbered as in its fullest form, skipping those its values remove (`kernel::primitive_numbers`, `ElementMap::primitive`). A cone is always Face1 its side, Face2 its top cap, Face3 its base cap; Edge1 the top circle, Edge2 the seam, Edge3 the base circle; Vertex1 and Vertex2 the circles'. A pointed cone's apex, which the fullest form hasn't, is numbered after: Edge4 and Vertex3 at the top, Edge5 and Vertex4 at the base. A pointed cone has no Face2, so a reference to its old top rim fails, `reference_broken`, and never re-points. No other primitive's elements depend on its values (a torus whose tube would cross the axis is refused; a horn torus has a torus's elements), so theirs are OCCT's numbers, and FreeCAD's fixtures, all full primitives, are unchanged. The fake numbers alike: it shares the map. FreeCAD's form (`naming::freecad::text`) writes these numbers too, so for a pointed cone it isn't FreeCAD's own name (FreeCAD's `Edge1`, OCCT's first, is Silico's `Edge4`), and FreeCAD's orders, which compare that text, are taken on Silico's numbers; for every full primitive, which is all FreeCAD's fixtures hold, the two agree. Tested on the real kernel by where each element is (silico-kernel `tests/primitive_numbers.rs`).

### The map

`naming::ElementMap`: each element's names (its own first) and each name's element, every name unique, every face, edge, and vertex named. A primitive's map lists its elements, each by its number as in the primitive's fullest form (`ElementMap::primitive`, `kernel::primitive_numbers`, above). An operation's is built by `ElementMap::operation` from its inputs' maps and its history, in FreeCAD's steps 1 to 5 above: kept, derived, from above, from below, and then, where FreeCAD leaves an element unnamed, by its number. A move keeps its input's map.

Some names don't say which element they name independently of numbering, and a reference to one is refused (N3, M2.8): `ElementMap::is_unstable` says which. Two elements that come about alike are numbered in FreeCAD's element order (`~1`, `~2`), as FreeCAD's debug builds number them; an operation's element named by its number is unstable too; and so is any name derived from one of those, in the solid it's in or any made from it.

Every name is bounded: at most 256 levels deep, and at most 4,096 names written out (a name used twice counts twice). Every walk over a name (writing it, comparing it) is as long as the name written out, and the review found a worker history that doubled a name with every feature and a timeline deep enough to overflow the stack. Past a bound, an element is named by its number, so it's unstable. FreeCAD bounds the same growth with its string table, which Silico doesn't have.

The kernel reports with each history the result's topology (`History::topology`: each face's edges and outer-wire edges, each edge's vertices, in OCCT's order), which the upper and lower passes read: workers report raw facts, and the core decides.

A model names a feature's elements the first time they're asked for (`Model::element_map`, naming every feature before it not yet named): only references and queries need names, and naming every feature at publishing doubled the cost of a long timeline on the fake. Each feature has a naming key, of its step, its ID, how it names, and its inputs' naming keys; the engine keeps the maps named so far by key, for the last two published models, so a feature named alike isn't named again, and an edit names only what it changes. A renamed feature renames its elements without a kernel call. Maps aren't written to the cache; the histories they're built from are.

### Not yet

- Extrusions and revolves (M3, with sketches): FreeCAD names an extrusion's top from the whole solid its profile generates (`;:G0`) and a partial revolve's end faces from their edges. Silico will take the ends from `History::ends`, and port FreeCAD's second round for the rest.
- References and resolution: M2.8.

## Tests

FreeCAD's tests, ported (silico-kernel `tests/naming.rs`, FreeCAD's expected names verbatim in `tests/fixtures/naming/`, compared without tags but with the sources in parentheses and the `K` positions, which FreeCAD's own matchers drop; an exact comparison, so it holds what the matchers' comparison with positions stripped would, and more):

| FreeCAD test | Status |
|---|---|
| `TopoShapeExpansion.cpp` `makeElementBooleanCut` (38 names), `makeElementBooleanFuse` (66), `makeElementBooleanCommon` (`Face1 -> Face3;:M;CMN`), `makeElementChamfer` (98), `makeElementFillet` (106; FreeCAD checks a subset, Silico all) | Ported, every name exactly, each element one name. |
| `makeElementTransformWithMap` | Ported: a move reports each element as itself, so the map is kept. |
| `TestTopologicalNamingProblem.py` `testPartDesignElementMapChamfer`, `...Fillet` (10 mm box, size 1: 98 and 106 names) | Ported: the names are the unit cube's. |
| `testPartDesignElementMapBox`, `...Cylinder`, `...Sphere`, `...Cone`, `...Torus` (26, 8, 6, 8, 4) | Ported: a primitive names each element, seams and degenerate edges too. |
| `testBodySubShapeBinderElementMap` (coincident boxes fused: 26 names, from both, the first operand first) | Ported (the fuse; the binder parts wait for binders). |
| `testGetElementFunctionality` (a name and its index find the same element) | Ported in every map test. |
| `TopoShapeTest.py` `testPartFuse`, `testPartCommon`, `testPartCut`, `testTopoShapeFuse`, `...Common`, `...Cut`, `testTopoShapeMakeFillet`, `...MakeChamfer` (58, 26, 26, 42, 42 names) | Ported: the map sizes, one name per element. |
| `tests/src/App/MappedElement.cpp` comparator tests | Ported, those without `#` (`naming::freecad`). |
| `makeElementPrism`, `makeElementRevolve`, pad and revolution tests | M3, with extrusions. |
| `testPartDesignTNPChamfer`, `testPartDesignTNPFillet`, `testBaseFeatureAttachmentSupport` | M2.8 (references surviving edits). |
| `MapperMaker*`, `MapperHistory*`, `StringHasher`, child-map and persistence tests | Not ported: FreeCAD's internals Silico doesn't have. |

Silico's own, for N7 (silico-kernel `tests/name_collisions.rs`, `tests/naming.rs`; silico-core `tests/naming.rs`): a hole's rims the same with no earlier hole or any; a slot's face and edge pieces ranked and the split names gone; a piece's name through a hole before the split and one after; a groove across a U face (three pieces); a ball's through hole and a pin through a torus (rims, loops, and seam points ranked, each referenceable); the two cubes' fuse with positions on its 8 split edges' pieces alone, each rank FreeCAD's position; a fuse, a slot, and a notch naming everything else alike with and without a hole before them; a face kept and modified too, a `~1`, both unstable; and the ball, torus, and groove in the two-pools-and-cache test.
