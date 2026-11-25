# The plate (M6.5's done test)

The spec an agent is given, with nothing but the MCP server (docs/roadmap/m6-features.md, *The plate*; docs/design/m6.0-patterns.md, section 2.2):

> Design a mounting plate in `plate.silico`, in millimetres, every sketch fully defined:
>
> - **Plate:** 100 (x) by 60 (y), centred on the origin, sketched on `xy` and padded 5 up.
> - **Notches:** a 6 × 10 notch at x 44 to 50, y −5 to 5, pocketed through, and mirrored about `yz` for the other end.
> - **Corner holes:** a hole r 2.75 at (−40, −20), pocketed through, and patterned along x (an extent of 80, 3 holes) and y (an extent of 40, 2 holes): six holes.
> - **Boss:** r 8, sketched on the plate's top face at its centre, padded 10.
> - **Boss holes:** a hole r 1 at (5, 0), pocketed up through the boss and the plate, and patterned 4 times about z over 360°.
> - **Chamfer:** the top rim of the corner hole at (40, 20), 0.5.
>
> Work in a transaction. Read each pattern to check where its copies are. Commit, check the model, and measure its volume. Then try two edits and roll each back: the plate padded 6 thick, and the corner holes 2 along x, not 3. Export it as `plate.3mf` (0.1 mm) and `plate.step`.

## By hand

| Feature | Volume |
|---|---|
| The plate, 100 · 60 · 5 | +30,000 |
| Two notches, 6 · 10 · 5 each | −600 |
| Six corner holes, π · 2.75² · 5 = 37.8125π each | −226.875π |
| The boss, π · 8² · 10 | +640π |
| Four boss holes through 15, π · 1² · 15 each | −60π |
| The chamfer on a rim of radius 2.75: a ring of section 0.5²/2, its centroid 0.5/3 out from the rim, by Pappus 2π · (2.75 + 0.5/3) · 0.125 | −0.25π · (2.75 + 0.5/3) |
| **In all** | **29,400 + 352.395833π ≈ 30,507.084161 mm³** |

- **The plate 6 thick:** 36,000 − 720 − 6 · 45.375π + 640π − 4 · 16π − 0.25π · (2.75 + 0.5/3) = 35,280 + 303.020833π ≈ 36,231.968024 mm³; the boss sits on the top face, now z = 6, and the chamfer's rim is found again by its name.
- **The corner holes 2 along x:** the copy (2, 1) is gone, so the chamfer's rim, `C2,1`'s, is `reference_broken`, with candidates, none taken.
- **The pattern reads:** the corner holes step 80 / 2 = 40 along +X and 40 / 1 = 40 along +Y from the hole at (−40, −20, 0), so instance (2, 1)'s anchor is (40, 20, 0); the notch, centred at (47, 0, 0), is mirrored in x = 0 to (−47, 0, 0); the boss hole at (5, 0, 0), turned 90° at a time about +Z, lands at (0, 5, 0), (−5, 0, 0), and (0, −5, 0).
- **The rim chamfered:** a circle r 2.75 centred at (40, 20, 5), the top of copy (2, 1).
- **Bounding box:** [−50, −30, 0] to [50, 30, 15].
