# Timeline edits: insert, move, and remove

What FreeCAD's PartDesign Body does when a feature is inserted, removed, or moved in a body, and what Silico's `document::chain` does instead. Silico's rules are the M5.0 design's (docs/design/m5.0-app.md: section 7's chain, A9, and section 12's items 1, 2, 2b, and 5). This note maps FreeCAD's behavior and its fixes to them.

## Sources

- `src/Mod/PartDesign/App/Body.cpp`: `insertObject`, `setBaseProperty`, `removeObject`, `getPrevSolidFeature`, `getNextSolidFeature`.
- `src/Mod/PartDesign/Gui/CommandBody.cpp`: `CmdPartDesignMoveFeatureInTree::activated` and `CmdPartDesignMoveFeature`.
- `src/Mod/PartDesign/App/Feature.cpp`: `onBaseFeatureRerouted`, `relinkToMatchingSubelements`.
- `src/Mod/PartDesign/PartDesignTests/TestFillet.py`.

## What each does

| Edit | FreeCAD | Silico |
|---|---|---|
| Insert | `insertObject` places the feature in the group, and `setBaseProperty` sets its `BaseFeature` to the previous solid feature and the next solid feature's to it. | The client's `base` is kept. Each input the feature consumes is taken from its old consumer, which must come at or after the place. A boolean whose base and tool both had consumers, or whose base is its tool, is refused (A9). |
| Remove | `removeObject` points the next solid feature's `BaseFeature` at the previous solid feature, only where it named the removed one. `onBaseFeatureRerouted` then re-points a dress-up's edges where an edge has exactly one geometric match. | The removed feature's consumer takes its base, or none for a pad's or revolve's. It is refused, naming both features, where nothing can stand in (A9) and where the removal would leave a stored name's maker out of its base's lineage (2b). Names are never re-pointed. |
| Move | A removal and then an insert at the target, with the move refused where an earlier feature would depend on a later one ("Early feature must not depend on later feature"). | A removal and then an insert, refused before the feature's sketch, tool, or a maker of its names; after a feature using it as its sketch or naming its elements; where it would become a boolean's tool; and where a stored name's maker would leave its base's lineage (2b). |

## Bug fixes from FreeCAD's history

| Commit | Fix | In Silico |
|---|---|---|
| `26c895c30d` | A feature under a dress-up removed: the dress-up's edges are relinked to the new base where each has a unique geometric match, and left broken where not (`relinkToMatchingSubelements`). | Rerouted by remove and move (A9). An element that only passed through the removed feature keeps its source's name, so the name is found on the new base; FreeCAD's safe case needs no relinking. An element the removed feature made is refused (the removal names both) where FreeCAD leaves a broken link. |
| `24b9cdb4f9` | Only the next solid feature whose `BaseFeature` names the removed feature is rerouted, so a broken neighbour isn't given an invalid base. | The same: only the removed feature's consumer, by the one-consumer rule, is rerouted. |
| `11a93a0578` | Moving a feature checks that no earlier feature depends on a later one. | The sketch, tool, and maker checks in both directions, the lineage rule, and the gate after every edit. |
| `1c8ca27f28` | Several features moved at once keep their order. | Not applicable: one feature moves at a time. |

Tip handling (`e3d63493c7`, `4246c902a5`), a body's external `BaseFeature`, and `relinkToOrigin` have no counterpart in Silico's model.

## Tests

| FreeCAD test | Status |
|---|---|
| `TestFillet.testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge` | Ported: `removing_the_fillets_base_builds_it_on_the_base_before_its_name_found_there` (the bracket's fillet rebuilt on `h0000001`, its name found there) |
| `TestFillet.testDeletingPreviousFeatureDoesNotRelinkUnsafeBaseEdge` | Ported as Silico's refusal: `removing_the_feature_that_made_a_fillets_edge_is_refused_naming_both` |
