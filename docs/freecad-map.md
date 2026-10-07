# FreeCAD source map

Where to find FreeCAD's implementation of each Silico subsystem. Paths are relative to `FreeCAD/`, the local reference checkout, which is gitignored and has full history. Baseline commit: `876271d660` (2026-10-04).

FreeCAD keeps tests in two places. Python tests are in `src/Mod/<Module>/<Module>Tests/` and C++ tests are in `tests/src/Mod/<Module>/`.

| Silico subsystem | FreeCAD code | Notes |
|---|---|---|
| Topological naming | `src/App/ElementMap.*`, `src/App/MappedName.*`, `src/App/MappedElement.*`, `src/App/StringHasher.*`, `src/Mod/Part/App/TopoShapeExpansion.cpp`, `src/Mod/Part/App/TopoShapeOpCode.h`, `src/Mod/Part/App/TopoShapeMapper.*` | `TopoShapeExpansion.cpp` builds names from OCCT history. This is the most important file to study. |
| Shape wrapper and cache | `src/Mod/Part/App/TopoShape.*`, `src/Mod/Part/App/TopoShapeCache.*` | |
| Document, objects, properties | `src/App/Document.*`, `src/App/DocumentObject.*`, `src/App/PropertyStandard.*`, `src/App/PropertyLinks.*`, `src/App/PropertyUnits.*` | Recompute and dependency graph logic live in `Document.cpp`. |
| Transactions and undo | `src/App/Transactions.*`, `src/App/TransactionalObject.*`, `src/App/AutoTransaction.*` | |
| Expressions | `src/App/Expression.*`, `src/App/ExpressionParser.h`, `src/App/Expression.y`, `src/App/PropertyExpressionEngine.*` | |
| Units and quantities | `src/Base/Unit.*`, `src/Base/Quantity.*`, `src/Base/UnitsSchema*` | |
| Spreadsheet parameters | `src/Mod/Spreadsheet/App/Sheet.cpp`, `Cell.cpp`, `PropertySheet.cpp` | |
| Sketch object and solving | `src/Mod/Sketcher/App/SketchObject.cpp`, `Sketch.cpp`, `Constraint.cpp`, `SketchAnalysis.cpp` | `SketchObject.cpp` is huge and holds most of the edge cases. |
| Sketch solver (link, don't port) | `src/Mod/Sketcher/App/planegcs/` | |
| Sketch-to-face conversion | `src/Mod/Part/App/FaceMaker*.cpp` | Handles nested profiles, holes, and islands. |
| Attachment (sketch planes, datums) | `src/Mod/Part/App/Attacher.*` | |
| Feature-based modeling | `src/Mod/PartDesign/App/Feature*.cpp` | Pad, Pocket, Revolution, Groove, Fillet, Chamfer, Draft, Thickness, Hole, Loft, Pipe, Helix, patterns, Boolean |
| Primitives and booleans | `src/Mod/Part/App/PrimitiveFeature.*`, `FeaturePartBox.*`, `FeaturePartBoolean.*`, `FeaturePartFuse.*`, `FeaturePartCut.*`, `FeaturePartCommon.*` | Silico merges Part and PartDesign into one timeline, so check both modules for every feature. |
| Assembly | `src/Mod/Assembly/App/AssemblyObject.cpp`, `AssemblyUtils.cpp`, `src/Mod/Assembly/JointObject.py` | Joint logic is partly in Python. |
| Assembly solver (link, don't port) | `src/3rdParty/OndselSolver/` | |
| Measure and mass properties | `src/Mod/Measure/App/` | |
| STEP import and export | `src/Mod/Import/App/ReaderStep.*`, `WriterStep.*`, `ImportOCAF2.*`, `ExportOCAF2.*` | The `*2` versions are the current implementations. |
| STL and 3MF export | `src/Mod/Mesh/App/Core/MeshIO.*`, `src/Mod/MeshPart/App/Mesher.*` | |

## Mining the history

```bash
git -C FreeCAD log --oneline --follow -- <file>                       # history of a file
git -C FreeCAD log --oneline -i --grep='crash\|segfault\|hang' -- <file>  # bug fixes
git -C FreeCAD log --oneline -S'<symbol>'                             # when a symbol or condition was added
git -C FreeCAD show <hash>                                            # read a fix
```

## Running FreeCAD headless

```bash
/Applications/FreeCAD.app/Contents/Resources/bin/freecadcmd /tmp/script.py
```
