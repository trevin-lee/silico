# Hostile input catalog

Inputs that break CAD code. `break-it` tries every category that applies to its target, and appends a one-line entry whenever it finds a new kind of break.

## Numbers

- `0`, `-0.0`, negative values, `NaN`, `+inf`, `-inf`, subnormals, `f64::MAX`, `f64::MIN_POSITIVE`
- Values exactly at a tolerance and one ULP either side of it (OCCT `Precision::Confusion()` = 1e-7, `Precision::Angular()` = 1e-12)
- Very large coordinates (1e6 mm or more from the origin) where float precision runs out
- Extreme scale ratios: a 1e-5 mm feature on a 1e5 mm part
- Angles of 0, 360, 360 + ε, -ε, and 2π + ε. Degree values passed where radians are expected (90 as radians).
- Integer counts of 0, 1, `u32::MAX`, and negative counts passed through a signed type
- Vectors with finite components whose length overflows (`[1e300, 0, 1e300]`): normalizing gives `(0, 0, 0)`, and the error blames the wrong parameter

## Sketch geometry

- Zero-length lines, zero-radius circles, arcs with 0° or 360° sweep
- Coincident points, duplicate overlapping edges, collinear chains
- Self-intersecting profiles (a bowtie), profiles that touch themselves at a single point
- Open profiles, almost-closed profiles with a gap just above tolerance
- Nested profiles: holes, islands inside holes, a hole touching the outer boundary
- Tangent circles, internally tangent arcs, near-tangent geometry (a gap of 1e-9)
- Two long sides crossing at a very shallow angle (1e-8 rad over 100 mm): a closest-point formula using `uu·vv − uv²` cancels to noise and misses the crossing
- B-splines with repeated control points, degree higher than the number of control points, or periodic closure
- Thousands of entities, or a sketch entirely outside its own bounding region

## Sketch solver

- Over-constrained, redundant, and conflicting constraint sets
- Under-constrained sketches under drag, dragging to `NaN` or infinity
- Constraints that reference deleted geometry
- Near-singular systems (almost-parallel lines given a distance constraint)
- Circular expression references (`a = b + 1`, `b = a`)
- 10,000 or more constraints

## Features

- Pad or extrude with length 0, a negative length, or a direction parallel to the sketch plane
- Pocket that removes nothing, removes everything, or splits the body into two solids
- Fillet radius larger than the adjacent face allows, exactly equal to the wall thickness, or applied to every edge of a cube
- Chamfer across a chain of tangent-continuous edges
- Shell thicker than the part, or shelling with every face removed
- Booleans of identical shapes, coplanar touching faces, edge-only or vertex-only contact, an empty operand
- Boolean operands offset by 1-3x `Precision::Confusion()` (1e-7, 2e-7) so parallel faces or curved surfaces nearly coincide: OCCT can miss the intersection and return a valid-looking wrong solid (check OCCT warnings such as `BOPAlgo_AlertAcquiredSelfIntersection`)
- Booleans of two equal tori offset sideways, and other curved pairs (cone on a cylinder's base with axes 1e-7 apart): check volumes against an independent Monte Carlo point-membership model, since inclusion-exclusion holds even when OCCT treats distinct operands as one
- The same object passed as both operands, or operands sharing geometry (a shape and its moved copy), against the same shapes read separately from bytes: results and history must match
- Revolve with an axis that crosses the profile, or angles of 0°, 360°, or more than 360°
- Revolve a side leaning just off parallel to the axis (its length within 1e-7 of its length along the axis): OCCT makes a cylinder, not a cone, and calls the solid valid
- Revolve with a side or corner exactly 1e-7 from the axis, or across it by less than 1e-7: OCCT throws (`NCollection_Sequence::Value`, `BRep_API: command not done`)
- Revolve by less than `Precision::Angular()` (1e-12 rad) far enough from the axis to move the profile, and by 360° − ε, which OCCT sweeps as a partial turn whose ends coincide
- Patterns with count 0, count 1, count 100,000, or instances that overlap each other
- Mirroring across a plane that passes through the body
- Features applied to an empty or invalid base shape
- Transforms chained many times, including a move and its inverse: location chains that grow without bound, and bytes that differ from the original
- Measured volumes of results with curved faces trimmed by curves: compare with a closed form (sphere lens), as fixed-order integration is off by 1e-4
- Measured volumes of slender slanted prisms (travel 1e6 times their width) and tiny features far from the origin: each face's integral dwarfs the volume and cancels; check flat-faced solids against their mesh
- Meshes of thin valid solids (100 × 100 × 1e-6, or 1.1e-7 thick 10 mm from the origin): meshing fails, or gives faces too few triangles that still pass validation

## Feature tree and naming

- References to a deleted feature, a deleted sketch edge, or a face that an edit removed
- Reordering features to create a dependency cycle; inserting a feature before something it references
- Deleting a feature in the middle of the tree
- Two features with the same display name
- Features that produce zero faces or an empty result
- Long undo/redo sequences mixed with recomputes
- References by stale names after an upstream edit

## Files

- Empty, truncated (cut at every byte offset), wrong version, future version
- Unknown keys, missing required keys, duplicate IDs, dangling references, cyclic dependencies
- Unicode in names: right-to-left text, emoji, combining characters, null bytes, 10 MB names
- CRLF line endings, a byte-order mark, invalid UTF-8
- Very deep nesting, 100 MB documents
- External references that use path traversal (`../../etc/passwd`) or point to missing files

## MCP and agent input

- Malformed JSON, wrong types, missing fields, unexpected extra fields, out-of-range enum values
- Huge strings and arrays
- Many concurrent calls, calls made during recompute, cancellation partway through an operation
- Element references by stale, misspelled, or made-up names
- Calls in the wrong order (editing before opening a document, committing a transaction that was never started)

## Kernel worker and IPC

- Worker killed with `kill -9` partway through an operation
- Worker returning garbage, a truncated message, or a reply to a different request
- Slow worker (timeout path); worker that never replies
- Operations that use up memory (a pattern with 1e6 instances, a huge mesh export)
- Many concurrent requests, worker restart during a burst of requests
- A result that arrives after the request was superseded

## Import

- Malformed STEP, STL, or 3MF: truncated files, wrong units, inverted normals, non-manifold meshes, zero-area triangles
- Valid STEP files with features OCCT handles badly: very large assemblies, tiny sliver faces, self-intersecting surfaces
