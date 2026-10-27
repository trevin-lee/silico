# The bracket (M4.9's done test)

The spec an agent is given, with nothing but the MCP server:

> Design an L-shaped mounting bracket in `bracket.silico`, in millimetres:
>
> - **Base leg:** 60 long (x), 40 wide (y), 5 thick (z), lying on z = 0, from x = 0 to 60.
> - **Upright leg:** 5 thick (x, from x = 0 to 5), 40 wide (y), standing from the base to z = 50.
> - **Holes:** two M5 clearance holes (5.5 diameter) through each leg, 16 apart, centred across the width: in the base at x = 40, y = 12 and 28; in the upright at z = 35, y = 12 and 28.
> - **Inside corner:** filleted, radius 5, along the whole width.
>
> Work in a transaction. Check the model, render it from an isometric view, measure its volume and one hole's distance from the base's end, and export it as `bracket.3mf` for printing.

## By hand

- Legs: 60 · 40 · 5 + 5 · 40 · 45 = 12,000 + 9,000 = 21,000 mm³.
- Holes: 4 · π · 2.75² · 5 = 151.25π mm³.
- Fillet (a concave corner rounded adds material): (1 − π/4) · 5² · 40 = 1,000 − 250π mm³.
- Volume: 22,000 − 401.25π = 20,739.4359... mm³.
- Bounding box: [0, 0, 0] to [60, 40, 50].
- A base hole's centre (40, 12) to the base's end face x = 60: 20 (`center_to_element`); its rim to it: 20 − 2.75 = 17.25 (`distance`).

## The recorded session's wrong edit

The agent first pads the profile 4 mm instead of 40, sees the volume (2,100 mm³ for the legs) disagree with the spec, and rolls back.
