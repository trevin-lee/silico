# Suppression

What FreeCAD's PartDesign does with a suppressed feature, and what Silico's suppression does instead (M5.1). Silico's rules are the M5.0 design's (docs/design/m5.0-app.md: section 7's `suppress_feature`, A10, and section 12's items 3 and 4). Its behaviour follows SolidWorks (docs/ux/solidworks.md, §5, *Editing upstream*): suppressing a parent suppresses its children, unsuppressing the parent leaves them suppressed, and unsuppressing a child unsuppresses its parent. This note maps FreeCAD's behaviour and its history to those rules.

FreeCAD's source here is `~/Git/FreeCAD` at `876271d660` (main) and the `1.1.3` tag (`145529fe74`), the oracle's release. Line numbers are `113:` for 1.1.3 and `main:` for main. PartDesign paths are under `src/Mod/PartDesign/App/`. Results marked *observed* are runs of the oracle's FreeCAD 1.1.3 (`freecadcmd`), by the scripts of this note's study.

## Sources

- `src/App/SuppressibleExtension.{h,cpp}`: the `Suppressed` property.
- `Feature.{h,cpp}`: `Feature::recompute`, `getBaseTopoShape`, `onChanged`, `updateSuppressedShape`.
- `FeatureExtrude.cpp`, `FeatureSketchBased.cpp`: a pad or pocket on a null base.
- `FeatureDressUp.cpp`: `getContinuousEdges`, a fillet's or chamfer's edges.
- `FeatureTransformed.cpp`: `getOriginals`, patterns.
- `src/Gui/ViewProviderSuppressibleExtension.cpp` (1.1.3's context menu), `src/Gui/CommandFeat.cpp` (main's `Std_ToggleSuppress`).
- `src/Mod/PartDesign/PartDesignTests/TestSuppressed.py` (main only).

## What FreeCAD does

### The property

`Suppressed` is an `App::PropertyBool` of `App::SuppressibleExtension` (`113:src/App/SuppressibleExtension.h:34,47`, default false, "Is object suppressed"). `PartDesign::Feature` is the only C++ class with it in 1.1.3 (`Feature.h:28,50`, `Feature.cpp:86`); a sketch isn't one (`Sketcher::SketchObject` is a `Part::Part2DObject`), so a sketch can't be suppressed (observed: `sketch.Suppressed = True` raises `AttributeError`). It began as a property of `PartDesign::Feature` itself (`4d929d3301`), hidden, and moved to the extension (`5a22009217`, `677cadee5e`); `ecf7e51ab3` stopped hiding it.

### A suppressed feature's shape is its base's

`Feature::recompute` (`113:Feature.cpp:90-122`):

```cpp
if (!Suppressed.getValue()) { return Part::Feature::recompute(); }
... Part::Feature::recompute() ... catch: FC_ERR("Failed to recompute suppressed feature " ...)
Shape.setValue(getBaseTopoShape(true));
```

- **Passed through.** Whatever the feature computes is discarded, and its `Shape` is its base's (`getBaseTopoShape(true)`, `113:Feature.cpp:379-418`), with the base's element map. So every feature after it builds on the base as if the suppressed feature weren't in the chain.
- **A suppressed root's shape is null.** `getBaseTopoShape(true)` gives a null shape when there's no base. The feature stays suppressed (observed: a suppressed root pad stays `Suppressed`, valid, its shape null; a body whose tip it is logs "Tip shape is empty", `113:Body.cpp:412`, and keeps its previous shape).
- **Still executed in 1.1.3.** The feature's own `execute` runs and its result is thrown away; a failure only logs (observed: a suppressed pocket of length 0 logs "Failed to recompute suppressed feature", is valid, and passes its base's 1000 mm³ through). On main it isn't run (`ea6d66bfa0`).
- **Its placement is kept** across suppression (`57b9a41335`, `SuppressedPlacement`).

### Recompute and dependencies

Nothing cascades. In 1.1.3 nothing but the feature itself, patterns, the view provider, and Python read `Suppressed`: `Body`, the tree, and `src/App` have no suppression logic. A feature after a suppressed one recomputes as usual (it's touched through its `BaseFeature` link) on the passed-through shape.

- **Observed:** a 10 mm cube pad, a 5 mm pad on top, and a pocket (r 1, depth 5) on the second pad: with the second pad suppressed, the pocket measures 1000 − 5π = 984.292037.
- **Patterns** drop suppressed originals (`Transformed::getOriginals`, `113:FeatureTransformed.cpp:116-138`, `79f9da420c`).
- **The GUI** suppresses one object at a time. 1.1.3's context menu has a checkable *Suppressed* item that flips the one object and doesn't recompute (`113:src/Gui/ViewProviderSuppressibleExtension.cpp:90-102`). Main's `Std_ToggleSuppress` (`200ce1ef4b`, `main:src/Gui/CommandFeat.cpp:232-284`) flips each selected object in one transaction, still with no cascade.

### Features that name a suppressed feature's elements

A dress-up whose base is suppressed builds on the base's base, its edges found there when they came from further up: observed, a fillet r 1 of a pad's edge, taken through a suppressed pocket, measures 997.853982, the 10 mm cube less (1 − π/4) · 10.

A fillet of an edge the suppressed feature made fails. Observed: a pocket's hole rim filleted at r 0.5, then the pocket suppressed:

1. `C#Fillet.Base missing element reference C#Pocket ;…E.Edge13` (the `FC_WARN` of `113:src/App/PropertyLinks.cpp:512-517`), and the reference becomes `?Edge13`;
2. the fillet fails, `Invalid edge link: …Edge13` (`DressUp::getContinuousEdges`, `113:FeatureDressUp.cpp:212-218`);
3. it keeps its old shape, so the body still shows the filleted result;
4. unsuppressing the pocket heals the reference and the fillet builds again.

### A null base

A suppressed root leaves the features after it on a null base. Pad and pocket share `buildExtrusion` (`113:FeaturePocket.cpp:120-129`, `113:FeatureExtrude.cpp:424`, `:739`, `:780-810`):

- **A pad** takes its prism alone.
- **A pocket of a set depth** becomes its tool solid alone. Observed: after a suppressed root pad, a pocket of length 5 and r 2 measures π · 4 · 5 = 62.831853, with the bounding box (3, 3, 5) to (7, 7, 10).
- **A through-all pocket** fails, `Base feature's TopoShape is invalid` (`113:Feature.cpp:408`, through `getThroughAllLength`, `113:FeatureSketchBased.cpp:925-942`). With no base at all and a sketch with no support, `No base set, no sketch support either` (`113:FeatureSketchBased.cpp:702`).

## What Silico does

The document holds `"suppressed": true` on a suppressed feature of any type, written after `name`, and nothing for one that isn't (open item 4). The reader refuses `"suppressed": false`, so a document has one spelling; `suppress_feature` and `set_field` take `false`, and remove the field.

- **Passed through, as FreeCAD.** A suppressed feature isn't built, and has no outcome but *suppressed*. A feature whose `base` or `tool` names a suppressed feature builds on its **effective base**: the nearest unsuppressed feature up that input's chain, by `base` links. Its names are its effective base's, so a fillet's edges that came from further up are found as FreeCAD finds them. Suppression never rewrites a reference in the text (section 7's chain).
- **Not executed.** Silico runs no step of a suppressed feature: main's behaviour (`ea6d66bfa0`), not 1.1.3's. Its build can't fail anything, since it has none; its values are still checked by the gate as every feature's are (a box of length 0 is refused, suppressed or not), since they make the document invalid whatever is built. A suppressed sketch isn't solved.
- **Bodies.** A body is the result of a built feature that nothing built consumes, by effective inputs. So with the bracket's first pocket suppressed, its pad is consumed (by the second pocket, through it) and the fillet is the one body.

### Departures, each SolidWorks' behaviour

Where FreeCAD leaves a feature failing because of a suppression, Silico suppresses it with its cause. A feature that can't build is suppressed, not left failing (§5: a child goes with its parent).

- **Dependents go with it.** A dependent of a suppressed `X` is suppressed with it, taken to a fixed point in timeline order:
  - a feature whose `sketch` is `X`;
  - a feature that stores an element name one of whose makers is `X` (a fillet's or chamfer's `edges`; every feature in the parsed name, a modified element counted as its source, as its name keeps its source's);
  - a feature that stores an element name one of whose makers reaches the feature's base only as `X`'s tool, `X` a boolean. A fuse keeps its tool's untouched elements under the tool's names, so a chamfer of a fused tool's edge names the tool, not the fuse; but the fuse suppressed passes only its base through, so the edge is gone. The maker is followed down its consumers to the feature's base (each solid is consumed once, so it's one way); the first suppressed feature on the way that consumes it as its `tool` is the parent. A cut and a common name what they keep of their tool as their own (`I` sources), so they're the name's makers already;
  - a feature with an input it needs whose chain has no unsuppressed feature: a pocket's, groove's, fillet's, chamfer's, or boolean's inputs, never a pad's or revolve's base, which is optional.

  FreeCAD's fillet of a suppressed pocket's rim fails with `Invalid edge link` and keeps a stale shape; Silico suppresses it. FreeCAD's through-all pocket on a suppressed root fails; Silico suppresses it. FreeCAD's pocket of a set depth on a null base becomes its tool alone; Silico suppresses it too, since it needs its base (a pocket alone cuts nothing).
- **A pad or revolve whose whole chain is suppressed builds alone,** as FreeCAD 1.1.3 builds it, and as SolidWorks keeps a boss that isn't another's child.
- **The gate refuses** a document in which a dependent of a suppressed feature is unsuppressed, naming both. A hand edit or a merge can't leave a feature failing for a suppression either. It takes the dependents to a fixed point, as `suppress_feature` does, so a hand edit's every dependent is named in one check, the deeper ones as having to be suppressed with the feature the text suppresses; and its suggestion is an edit of the text (add `"suppressed": true` to the dependent, or take it off that feature), which a document the gate refuses takes.
- **Unsuppressing a child unsuppresses its parents,** recursively: its sketch, each suppressed maker of a name it stores, each suppressed boolean whose tool a maker of its names reaches its base through, and, for an input it needs whose chain holds no unsuppressed feature, that chain's root. Its dependents stay suppressed. FreeCAD unsuppresses only the object clicked.
- **An edge the suppressed feature shortened is filleted whole.** A cube, a notch cut from the end of its Edge12, and a 1 mm fillet of that edge, which the cut keeps under the cube's name: 1000 − 12 − (1 − π/4) · 8 = 986.283185. With the notch suppressed, the fillet builds on the cube and fillets the whole 10 mm edge, 1000 − (1 − π/4) · 10 = 997.853982 (the real kernel's, by hand: `a_fillet_of_an_edge_a_suppressed_notch_shortened_fillets_the_whole_edge`). FreeCAD 1.1.3 fails the fillet instead ("Invalid edge link", observed), since its element name for the shortened edge carries the pocket's tag, which the base without it hasn't. Silico keeps the reference to an edge that still exists, as SolidWorks keeps a child's reference to its parent's edge when a feature between them is suppressed (behaviour follows SolidWorks); the name was the cube's all along.
- **Sketches can be suppressed.** A sketch's users go with it. FreeCAD has no `Suppressed` on a sketch; SolidWorks suppresses sketches.
- **No self-unsuppressing root.** `86e75037d9` once made a suppressed feature with an invalid base unsuppress itself; `2a541c9536` removed it, so neither 1.1.3 nor Silico does it.

### Fingerprints (A10)

A suppressed feature's stored fingerprint is carried as stored, never checked or rewritten: whatever the save, by command, by hand, or by a merge, it's written as the file holds it, and an edit of its values leaves it too. So suppressing a feature adds one line to the file, `"suppressed": true`, and its own lines are otherwise as they were (a fingerprint dropped would reflow a primitive's or a boolean's object onto one line). Suppressing or unsuppressing a feature reaches every unsuppressed feature that uses it, directly or through others, since each now builds on another base; and unsuppressing it reaches it, so its next save writes its fingerprint fresh, and a stale one shows as drift if its `suppressed` line is taken off by hand. A suppress then an unsuppress, each committed, gives the file byte for byte as it was. FreeCAD has no fingerprints; it keeps the B-rep, and a suppressed feature's is its base's.

### Reads

A read of a suppressed feature's solid (`elements`, `query`, `mass_properties`, `meshes`, naming it) is refused, `invalid_input`, saying it's suppressed. The timeline marks it `suppressed`, with `suppressed` true. The outline (`silico show`) writes `suppressed` before its type.

## Bug fixes from FreeCAD's history

| Commit | Fix | In Silico |
|---|---|---|
| `4d929d3301` | The `Suppressed` property: a suppressed feature's shape is its base's, where the base is valid | The effective base |
| `86e75037d9` | A suppressed feature with an invalid base unsuppresses itself | Not taken, as `2a541c9536` removed it |
| `e6fbc6b447` | Under the toponaming fix, always pass the base through, null without one (`SuppressedShape` added) | A suppressed root passes nothing; a pad after it builds alone, a feature that needs a base is suppressed |
| `2a541c9536` | The old branch removed, `86e75037d9`'s self-unsuppressing with it | The same |
| `57b9a41335` | Placement kept across suppression (#20205); the base's shape always set | No placement of a feature's own: nothing to keep |
| `79f9da420c` | Patterns drop suppressed originals | No patterns yet (docs/ports/patterns.md takes it when they come) |
| `aed5727ac3` | A crash on a view provider with no owner | No view providers |
| `ea6d66bfa0` | A suppressed feature's `execute` isn't run (main only) | Taken: a suppressed feature runs no step |
| `2ee19dfcce`, `200ce1ef4b` | The toggle recomputes; several objects toggled in one transaction (main only) | `suppress_feature` is one edit, rebuilt as every edit is |
| `f4be654473` | Suppressed items struck through in the tree, and `TestSuppressed.py` (main only) | The app dims them (section 9) |

## Tests

| FreeCAD test (`TestSuppressed.py`, main) | Status |
|---|---|
| `testSuppressChangesShape` | Ported: a 10 mm cube with a 1 mm fillet of one edge, on the real kernel: 1000 − (1 − π/4) · 10 = 997.853982 built, 1000 suppressed, 997.853982 again unsuppressed, each by hand (FreeCAD 1.1.3, observed: 997.8539816339743, 999.9999999999998, 997.8539816339743). Silico's asserts the volumes, where FreeCAD's asserts only that they differ and come back |
| `testSuppressedPersistsAfterReload`, `testUnsuppressedPersistsAfterReload` | Ported as the reader's round trip of `"suppressed": true`, its refusal of `false`, and a suppressed document saved and opened again |
| `testMultipleSuppressedFeaturesReload` | Covered by the bracket's both pockets suppressed, saved and checked |
| `testSuppressibleExtensionExists` | Not applicable: every feature type takes `suppressed` |
| `TestSuppressedStrikethrough` | Not applicable: a GUI test; the app's (M5.4) |

1.1.3 has no suppression tests. Its observed cases are pinned on the real kernel, by hand: a pocket through a suppressed pad cuts the pad beneath (1000 − 5π = 984.292037, `a_pocket_through_a_suppressed_pad_cuts_the_pad_beneath`), and the notch above, a departure. The design's oracle case (the bracket built in headless FreeCAD with its first pocket suppressed, its volumes compared) goes in `tests/oracle/`, which is protected, with the maintainer; until then the bracket's volumes are checked by hand on the real kernel.
