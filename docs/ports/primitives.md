# Primitives

The solid primitives (box, cylinder, sphere, cone, torus), ported in M1.3 as a kernel operation (`Op::MakePrimitive`, `silico_kernel::make`) and a typed core function (`silico_core::kernel::KernelWorker::make`). Full shapes only: FreeCAD's angle parameters (a partial cylinder, a sphere segment, a torus sector) and the prism skew angles come later, with the feature layer.

## What FreeCAD does

Two families build them: the Part workbench features (`src/Mod/Part/App/PrimitiveFeature.cpp`, `FeaturePartBox.cpp`) and the PartDesign additive primitives (`src/Mod/PartDesign/App/FeaturePrimitive.cpp`). For full shapes they build the same geometry, and FreeCAD 1.1.3 gives identical results for both. Silico has one modeling workflow (ARCHITECTURE.md, invariant 7), so it ports that geometry once.

| Primitive | FreeCAD's construction | Placement |
|---|---|---|
| Box | `BRepPrimAPI_MakeBox(L, W, H)` | corner at the origin, along +X, +Y, +Z |
| Cylinder | `BRepPrimAPI_MakeCylinder(R, H, 2π)`, then its `BottomFace()` extruded by `(0, 0, H)` (`PrismExtension::makePrism`, `c0e6debcfa`, `7185ee93f2`) | base disc at z = 0, axis +Z, seam at +X |
| Sphere | `BRepPrimAPI_MakeSphere(R, −π/2, π/2, 2π)` | center at the origin |
| Cone | `BRepPrimAPI_MakeCone(R1, R2, H, 2π)`; when `|R1 − R2| < Confusion`, `BRepPrimAPI_MakeCylinder(R1, H, 2π)` built directly (`1eb0444bd5`) | `R1` at z = 0, `R2` at z = H |
| Torus | `TopoShape::makeTorus`: a circle of radius `r` centered at `(R, 0, 0)` with axis +Y, edged from −π to π, made a face, and revolved 2π about Z (`a2082838bb`, then `c578f9747a`) | in the XY plane about +Z; the seam vertex at `(R − r, 0, 0)` |

Angles are converted as FreeCAD converts them (`Base::toRadians`: `degrees * (π / 180)`), so OCCT gets the same bits.

**The construction is the port.** The same volume can come from a different construction with its faces and edges in another order. OCCT's own `MakeCylinder(R, H)` numbers the cylinder's faces cylinder, top, bottom, where FreeCAD's extrusion gives cylinder, bottom, top; OCCT's `MakeTorus` puts the seam vertex at `(R + r, 0, 0)`. FreeCAD's `Face1`, `Edge1`, ... are what people and scripts name, so Silico builds each primitive as FreeCAD does. `primitives_match_freecad_face_for_face_and_edge_for_edge` compares every face and edge, in order, with FreeCAD's Part features.

## Validation

FreeCAD checks some values, clamps others when Python assigns them, and lets OCCT reject or quietly mishandle the rest. Silico checks every value in one place (`Primitive::check`, in `silico-protocol`): the core refuses a bad value before anything is sent (`invalid_params_never_reach_the_worker`), and the worker checks again before OCCT sees it. Each refusal is `invalid_input`, naming the primitive and the parameter.

| Rule in Silico | FreeCAD | OCCT |
|---|---|---|
| Every length above 1e-7 (`Precision::Confusion()`) and at most 1e6 (a kilometre) | Part: `< Confusion` is an error. PartDesign clamps to 2e-7 and builds. Neither checks NaN or infinity. | `MakeBox` rejects `<= Confusion` with an empty `Standard_DomainError`; cylinders, spheres, and tori have no size checks. |
| A cone's radii are zero, or finite and above 1e-7 | `< 0` is an error (`b9e1a370e7`: zero must be allowed) | `0 < R < Confusion` throws "cone with negative or too small radius" |
| A cone's radii aren't both zero | Accepted; the result is an invalid zero-volume "solid" | Throws "cone with two identic radii" |
| A torus's minor radius at most its major radius, within 1e-7 | `r > R` fails in the revolve ("BRep_API: command not done"); `r = R` builds a horn torus | `MakeTorus(10, 12)` builds a self-intersecting torus that passes `BRepCheck` |

A result is checked like any operation's (docs/ports/fillet.md): one that isn't a valid solid is refused as `invalid_result`, never returned. No input within the allowed sizes is known to reach that check: a sphere of radius 1e300, which OCCT builds with three faces, is refused before as `invalid_input`, past the kilometre limit (`tests/bridge.rs`).

### Where Silico differs on purpose

All of these are inputs FreeCAD accepts and builds something broken from, or clamps without saying:

- **A length of exactly 1e-7.** FreeCAD's Part check is `< Confusion`, so 1e-7 passes it; a sphere or cylinder of radius 1e-7 comes out invalid ("Unorientable shape"), and a box of side 1e-7 fails in OCCT with an empty message. Silico refuses lengths at or below 1e-7.
- **NaN.** Passes every FreeCAD check; a NaN box comes out invalid with volume 0. Refused.
- **Infinity and huge sizes.** FreeCAD clamps to `FLT_MAX` or `DBL_MAX`, giving "Infinite parameter" or an invalid three-faced sphere. Past about 1e7, OCCT also builds some solids that pass its validity check but measure wrongly (a torus of radii 1e12 and 1e-3 has a negative volume). Silico refuses lengths above 1e6 (a kilometre), and refuses an invalid result.
- **A cone with no radius.** FreeCAD builds an invalid shape; Silico refuses the values.
- **Negative values.** FreeCAD's Python property assignment clamps a negative length to 0 (then the size check fails), or, in PartDesign, to 2e-7, silently building a microscopic solid. Silico refuses a negative value as given.
- **A cylinder of height 1e-7.** OCCT builds a "valid" capless shape. Refused (at or below 1e-7).

### Same as FreeCAD

- Equal cone radii, or radii within 1e-7 of each other, make a cylinder of radius `R1`, built directly (so it's numbered like OCCT's cylinder, not like the cylinder primitive). Checked against the oracle for `(5, 5)` and `(5, 5.00000005)`.
- A cone may come to a point at either end (`R1 = 0` or `R2 = 0`).
- A horn torus (`r = R`, or `r` within 1e-7 above `R`) builds; its face is a surface of revolution, not a toroid. Checked against the oracle.

## FreeCAD tests

| Test | Status |
|---|---|
| `TestTopologicalNamingProblem.py` `testPartDesignElementMap{Box,Cylinder,Sphere,Cone,Torus}`: face, edge, and vertex counts (cylinder 3/3/2, sphere 1/3/2, cone 3/3/2, torus 1/2/1) | Covered by the oracle comparison, which checks every face and edge. The element map sizes are M2. |
| `src/Mod/Part/parttests/TopoShapeTest.py` `testTopoShapeBox` (2 × 1 × 2: area 16, volume 4, center (1, 0.5, 1), 12 edges, 6 faces, 8 vertices) | Covered by the box's analytic tests (`crates/silico-kernel/tests/bridge.rs`); the inertia matrix and static moments wait for a summary that has them. |
| `src/Mod/PartDesign/PartDesignTests/TestPrimitive.py` (each primitive minus a smaller one: 11³ − 10³, π·10·(11² − 10²), ...) | M1.5, with booleans. |
| OCCT's `BRepPrimAPI_Make*_Test.cxx` GTests | Not ported: they test OCCT, which Silico takes as given, and the oracle compares the results that matter. |

## Limits

- A horn torus (the tube touching the axis) far smaller than a millimetre is measured only roughly: OCCT can't resolve its singular point at its own tolerance's scale. Radii of 1e-7 give a volume about 8% off, 1e-6 about 2e-4, and 1e-3 or more exact to the last digits. It's a valid solid with the right faces and edges; only its measurements are approximate. Nothing Silico is for is that small.
- A torus whose radii span the whole allowed range (1e6 and 1.01e-7) measures within about 5e-4, and so does a cone (radius 1e6, height 1.01e-7) within about 1e-3. Every other combination of the extreme sizes (one parameter at 1e6 or 1.01e-7, the rest ordinary) measures within about 2e-9.
- The torus rule allows the tube to exceed the ring by up to 1e-7, OCCT's tolerance, as an absolute amount. At sub-micron sizes that's a large fraction: a ring of 1.5e-7 accepts a tube of 2.4e-7, which OCCT builds as a self-intersecting torus it calls valid. From a micrometre up the overshoot is negligible (9e-5 of the ring at 1e-3).

## Notes

- `Part.makeTorus`, the Python function, uses OCCT's `MakeTorus` and so differs from the `Part::Torus` feature (seam at `R + r`). The oracle uses the features, as a person's model does.
- OCCT 8 fixed partial tori with inverted heights (bug 23612); whether Silico's 7.9.3 has the fix is to be checked when angle parameters arrive.
- When the cone gets its angle, the equal-radii cylinder must take it too: FreeCAD built a full cylinder whatever the angle until `990b9b27fe`.
- The choice between a cone and a cylinder is made in Rust (`silico_kernel::make`); the C++ only builds what it's told.
- FreeCAD's PartDesign wraps each primitive in `FeaturePrimitive::execute`, which fuses it with the body's base shape when there is one. That's the feature layer's job (M2), with booleans (M1.5).
