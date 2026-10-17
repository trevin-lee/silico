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
4. **Upper pass** (`:1828-1904`): for (face, edge) then (edge, vertex), each named upper element's unnamed lower elements, counted in `TopExp::MapShapes(upper, lower)` order; each lower element takes the upper whose name sorts first by `ElementNameComparator` (`src/App/MappedElement.cpp:240-259`).
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

`naming::Name` is a structured value: an `Element` (a feature's own element by number: a primitive's, or the last resort), a `Derived` element (the operation, the result's kind, the sources each with relation and position, first by FreeCAD's key, up to five), an `Upper` element (from the element above, `nth`), a `Lower` element (from the elements below, up to five), or a `Duplicate` (`~n`). Written in Silico's form, the maker feature's ID, the kind, and how it came about:

| Silico | FreeCAD (without tags) |
|---|---|
| `k7f2qx3m.Face3` | `Face3` |
| `m3tq8v0z.Face(M a91cw0de.Face1)` | `Face1;:M;CUT` |
| `m3tq8v0z.Edge(G2 k7f2qx3m.Face6, G2 a91cw0de.Face1)` | `Face6;:G2(Face1;K-2);CUT` |
| `f.Edge(U2 f.Face(G e.Edge1))` | `Edge1;:G;FLT;:U2;FLT` |
| `f.Face(L a, b, c)` | `a;:L(b|c);FLT` |
| `<name>~1` | `<name>;D1` |

A kept element keeps its name exactly, through any number of features (N2): FreeCAD adds a tag level per feature. It's safe because a solid is consumed once (M2.1's D2), so an element's lineage is a tree and one origin can't reach a shape twice; patterns and mirrors, which copy, will add a copy marker. The op code isn't written (the maker feature says it) but is held, for FreeCAD's form and FreeCAD's orders.

`naming::freecad::text` renders FreeCAD's form without tags, for comparing with FreeCAD's names and for FreeCAD's orders (which source comes first, which element above or below names an element). Silico orders inputs by their place in the operation (base, then tool), where FreeCAD orders them by tag (object IDs, in creation order): the same in FreeCAD's tests and in documents built in order.

### The map

`naming::ElementMap`: each element's names (its own first) and each name's element, every name unique, every face, edge, and vertex named. A primitive's map lists its elements, each by its number (`ElementMap::primitive`, from the counts OCCT gives each primitive, `kernel::primitive_counts`). An operation's is built by `ElementMap::operation` from its inputs' maps and its history, in FreeCAD's steps 1 to 5 above: kept, derived, from above, from below, and then, where FreeCAD leaves an element unnamed, by its number. A move keeps its input's map.

Some names don't say which element they name independently of numbering, and a reference to one is refused (N3, M2.8): `ElementMap::is_unstable` says which. Two elements that come about alike are numbered in FreeCAD's element order (`~1`, `~2`), as FreeCAD's debug builds number them; an operation's element named by its number is unstable too; and so is any name derived from one of those, in the solid it's in or any made from it.

Every name is bounded: at most 256 levels deep, and at most 4,096 names written out (a name used twice counts twice). Every walk over a name (writing it, comparing it) is as long as the name written out, and the review found a worker history that doubled a name with every feature and a timeline deep enough to overflow the stack. Past a bound, an element is named by its number, so it's unstable. FreeCAD bounds the same growth with its string table, which Silico doesn't have.

The kernel reports with each history the result's topology (`History::topology`: each face's edges and outer-wire edges, each edge's vertices, in OCCT's order), which the upper and lower passes read: workers report raw facts, and the core decides.

A model names a feature's elements the first time they're asked for (`Model::element_map`, naming every feature before it not yet named): only references and queries need names, and naming every feature at publishing doubled the cost of a long timeline on the fake. Each feature has a naming key, of its step, its ID, how it names, and its inputs' naming keys; the engine keeps the maps named so far by key, for the last two published models, so a feature named alike isn't named again, and an edit names only what it changes. A renamed feature renames its elements without a kernel call. Maps aren't written to the cache; the histories they're built from are.

### Not yet

- Extrusions and revolves (M3, with sketches): FreeCAD names an extrusion's top from the whole solid its profile generates (`;:G0`) and a partial revolve's end faces from their edges. Silico will take the ends from `History::ends`, and port FreeCAD's second round for the rest.
- References and resolution: M2.8.

## Tests

FreeCAD's tests, ported (silico-kernel `tests/naming.rs`, FreeCAD's expected names verbatim in `tests/fixtures/naming/`, compared without tags but with the sources in parentheses and the `K` positions, which FreeCAD's own matchers drop):

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
