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
- Counts from a command line fed to `(0..n).map(..).collect()`: `collect` sizes the Vec up front, so `usize::MAX` panics (capacity overflow) and 1e15 aborts on allocation
- Derived measures in fixed point sized for their inputs' units (millionths of a mm³ in an i64 hold 9.2e12 mm³, a cube 21 m on a side, while the gate admits 1 km sides): take the largest values the gate admits through every derived quantity (volume, area, products of lengths)
- Absolute tolerances on quantities that scale with the part (OCCT's `GeomLib::NormEstim` orients a pole's or apex's normal only when \|dP/du × dP/dv\| a 1e-5 step away is over 1e-8): a ball under 0.0316 mm radius or a cone slenderer than 1:1000 gets a normal pointing into the solid
- Two rules composed into a third (a direction within 4ε of unit length times a length up to the limit, then checked as a vector at most the limit): the product lands a few ulps past it and is refused naming a parameter the caller never sent; search the boundary with `next_up`/`next_down`
- Refusals written as `change <= k * before`: NaN makes every comparison false, so the refusal is skipped; an infinite result or a negative (inverted) base passes too
- Angles from `atan2` at a signed zero (a point at its centre's left with y = −0 gives −π, +0 gives π): a request whose points are placed with zeros as 0 still differs by a turn, and the kernel's bytes with it
- One rule computed twice, by the core and again by a worker (a sliver's |area| ≤ tol × perimeter about different first points, sqrt against hypot): bisect a turned thin strip's width to the core's boundary and send it; a shoelace sum of coordinates ~10 cancels to ~1e-8 relative
- Relative no-op thresholds on large parts (a change of at most 1e-9 × the base's volume is "nothing"): a 4 × 2 mm hole through a 200 m block removes 1.6 litres and is refused, the message claiming the volumes equal
- Suggested values ("export with a tolerance of at least X") computed from a margin recovered by subtraction (tolerance − deflection) and checked against it, while the consumer subtracts the original rounding: an input landing an ulp past a rounding step (a whole nanometre) gets a suggestion an ulp short; check each suggestion with the consumer's own formula, and at the parameter's largest values (twice the largest tolerance is past the range)
- Two results compared by their coordinates' bits where the same module elsewhere takes -0 as 0 (joining nodes, writing f32): a node at -0 in one and 0 in the other is called a different result
- Directions a request gives that are valid but tiny or huge (1e-160, 1e-200, 1e200, `f64::MAX`), used without scaling: their squares underflow to subnormals (an angle off by thousandths of a degree) or to 0, or overflow to infinity, so a direction is refused as zero or answered wrong; scale by the largest component first

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
- Constraints that force geometry of no size through shared points (a line's two ends mirrored about the line itself, a circle's or arc's center on it, a line's midpoint at its own end, a line tangent to a circle centered on its end): FreeCAD's GUI refuses some, and its solve says success with a zero-length line or a radius of 0
- Degeneracy a check compares by ID that's there by value (a line between two distinct points at the same place), and values derived from checked ones past the solver's reach (an arc's radius from points each within a kilometre is up to 2√2 km)
- A symmetry of a line's own end and another point about that line (the gate refuses both ends, not one): the solve puts the point on the end, and the next solve refuses two points drawn at one place
- Profiles the core takes but the kernel request's check refuses (a corner arc of radius in (1e-7/√2, 1e-7], whose ends are r√2 apart; a loop of two curves whose shared points are within the tolerance, where no end is unshared for a neighbour test to see): send every profile the core makes through the request's check
- A lone curve whose own two ends are within the tolerance (a nearly closed arc, a 5e-8 line): "share the point" makes a curve from a point to itself, which the gate refuses; follow each refusal's suggestion and gate the result

## Sketch solver

- Over-constrained, redundant, and conflicting constraint sets
- Under-constrained sketches under drag, dragging to `NaN` or infinity
- Constraints that reference deleted geometry
- Near-singular systems (almost-parallel lines given a distance constraint)
- Circular expression references (`a = b + 1`, `b = a`)
- 10,000 or more constraints
- Tags or IDs past `i32::MAX` checked as `u32` and handed to C++ as `int`: PlaneGCS leaves negatively tagged constraints out of its diagnosis, so a conflict goes unnamed and the solve says Success
- Constraints on fixed values only (two fixed points, a dimension between them that doesn't hold): PlaneGCS solves only components with unknowns and says Success, even when it names the constraint conflicting
- Geometry drawn degenerate (a line from (1, 1) to (1, 1)) under a constraint whose gradient or scale divides by its length: the Jacobian row is NaN and a satisfiable constraint is called conflicting
- A solver's lists of parameters or tags read as sets: PlaneGCS lists a dependent parameter once per group, so check each is listed once
- Sizes the check bounds by the wrong product: Levenberg–Marquardt's JᵀJ is unknowns², not rows × unknowns (2,000 lines chained by parallels, half the Jacobian limit, runs for many minutes)
- Counts a limit misses because entries may share values (a million points on the same two values): bound every list, not only the values
- Solutions past the check's own reach (a point dimensioned 1e6 from one at 1e6): the solve succeeds and the next solve, started from it, is refused
- Drag frames taken as solved without building their geometry: a line's end dragged onto its other end (or a horizontal line's end to a cursor square above its start) is a Success with a line of no length the next solve refuses, an arc's end dragged onto its centre a radius of -8e-13; FreeCAD undoes such a frame (`Sketch::internalSolve`, `updateGeometry`)
- Drags of the largest system the check admits: the pin puts the component under SQP, dense unknowns² matrices every iteration (2,048 unknowns: 45 s a frame, the solve 1.9 s); limits sized by the solve don't bound a drag
- Constraints redundant only where a drag starts (a point's Distance and DistanceX from a fixed point, drawn at (5, 0)): the dropped one is never looked at again, and moves break it with Success (FreeCAD too); a redundant one unmet there makes every move fail
- Special cases that send fewer rows because "the rest is implied" (an arc's ends symmetric about a line its centre is held on: the midpoint row only): find the solution the reduced system has that the full one doesn't (a half circle, whose chord's midpoint is its centre), and draw toward it; any arc drawn past a half turn solves there with its ends unmirrored, Ok (FreeCAD too)
- A dropped redundancy that was the well-conditioned one (a vertical, where an equal length fixes the same x only through a square root): the solve lands √residual off (12 nm on a 1 mm side), and an agreement check blames the constraint PlaneGCS dropped
- Diagnoses on shared points against FreeCAD's coincident ends, at extreme redimensioning (a square template set to 100,000 × 0.01): FreeCAD finds one redundancy, Silico six conflicts; run the oracle on both
- Values written back rounded whose measure depends on the value they replace (a reference angle measured on its stored value's turn, `stored + remainder(measured − stored, 2π)`): near a rounding boundary each save flips the last digit; bias directions to within 1e-13 of a half unit, store them a turn away, and save three times
- Solutions checked by the solver's values while the file stores what those derive from (an arc's radius value against its points, which the gate re-measures): dimensioned to exactly the limit, a quarter of solves save points an ulp past it, and the file is refused when read
- A save's answer against its load's, whole: solve, write through the file, read, solve again, and compare Ok-ness, bits, and the redundant constraints named; an answer's outcome depends on where it was diagnosed, not only its values (a solution Ok from the drawing that PlaneGCS calls conflicting from itself), and a "solve again from the solution" that starts from the solver's values rather than what a load derives from the file (an arc's radius and angles from its points) isn't a load
- Redundancies that agree held by an angle at micrometre scale: the solver's absolute convergence on lengths (1e-10) is a relative 1e-8 on a 1.6 µm side, past a scale-free angle tolerance
- Tolerances taken over parts that the dropped row itself joins: a redundant row bridging two rigid, linearly held points far apart gets the bridged box's extent though PlaneGCS solved each apart exactly (5 nm kept at 10 m, refused at 1 mm); and per-part windows each recomputed over every constraint (512 parts × 9,000 rows: 0.4 s a read in release, 13× one part's)

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
- Revolve a profile that touches its axis only at a corner (a cone's apex where its slanted side meets the side on the axis, a diamond's tip): OCCT generates a degenerate edge from that vertex (two, when MakeRevol copies one two faces share), so a history rule of "on the axis, it generates nothing" refuses the commonest revolve
- Bases of several solids (a fuse of two boxes apart, which M2 accepts) under a feature that checks its result is one solid: the extra solid is blamed on the feature ("doesn't reach its base", "cuts its base in two")
- A sweep's vector, or a revolution's axis, in the faces' own plane, given to a kernel operation that leaves the plane to the core: OCCT builds a solid of volume 0 that `BRepCheck` calls valid
- OCCT makers that share their input's topology (`MakePrism`, `MakeRevol` without copying): they add curves and locations to the input's edges, so the input's bytes change and each later sweep of the same shape in one process writes different bytes; compare the input's bytes before and after, and sweep it twice
- Revolve with a side or corner exactly 1e-7 from the axis, or across it by less than 1e-7: OCCT throws (`NCollection_Sequence::Value`, `BRep_API: command not done`)
- Revolve by less than `Precision::Angular()` (1e-12 rad) far enough from the axis to move the profile, and by 360° − ε, which OCCT sweeps as a partial turn whose ends coincide
- Patterns with count 0, count 1, count 100,000, or instances that overlap each other
- Mirroring across a plane that passes through the body
- Features applied to an empty or invalid base shape
- Transforms chained many times, including a move and its inverse: location chains that grow without bound, and bytes that differ from the original
- Measured volumes of results with curved faces trimmed by curves: compare with a closed form (sphere lens), as fixed-order integration is off by 1e-4
- Measured volumes of slender slanted prisms (travel 1e6 times their width) and tiny features far from the origin: each face's integral dwarfs the volume and cancels; check flat-faced solids against their mesh
- Meshes of thin valid solids (100 × 100 × 1e-6, or 1.1e-7 thick 10 mm from the origin): meshing fails, or gives faces too few triangles that still pass validation
- Mesh checks done triangle by triangle or shell by shell (each shell's signed volume): a solid whose shell has no faces, or whose triangles all weld into one point, passes with no triangles; a cavity larger than its outer shell, or a lone shell flagged a cavity, passes with a negative total
- Parts thin beside their distance from the origin, written as 32-bit floats (a 1e-5 mm slab 1000 mm out): rounding flattens the solid, and whichever check runs first misnames the cause ("faces inward") and advises a finer tolerance, which makes it worse
- Mesh normals at a surface's singular points (a cone's apex, a sphere's pole, a horn torus's center, a fillet's corner ball): check each vertex normal is on the outward side of every triangle it's in. Skip slivers by aspect, not absolute area: three collinear nodes 1e5 from the origin have a cross product of 1e-11 pointing anywhere
- Fillet or chamfer an edge that ends where two solids touch at a single vertex (two cubes fused corner to corner): OCCT segfaults, as FreeCAD does
- Edges whose faces meet at a few degrees (a 3° crease, every side edge of a regular 63-gon or finer prism): OCCT calls normals within 0.1 rad tangent and won't fillet or chamfer them; check the error doesn't misdescribe the corner
- Sizes too small to cut (a distance-and-angle chamfer at 1e-9°) or a size OCCT fails at whatever it is: check the error's suggestion doesn't send the user the wrong way ("try a smaller size")
- Angles at a singularity: a distance-and-angle chamfer at exactly 90° on an acute edge gives an invalid solid one way round, while 89.99° and 90.01° are fine
- The same edges listed in another order: OCCT's chamfer of all 12 box edges at just under half the face builds listed 1 to 12 and fails listed 12 to 1, and a tangent chain takes its reference face from the first edge listed
- Fillets that collide only in pairs (two parallel edges on a face narrower than 2r) with a bystander edge listed before them: OCCT's faulty-contour report (`ChFi3d_StripeEdgeInter` in `ChFi3d_Builder::Compute`) names only the first contour of the colliding pair, which can be the bystander; check every edge a refusal names against a hand calculation of whose r × r strips overlap, and follow the "leave it out" advice to see what it costs
- Chamfers whose tangent chain runs past a face away from the edge given (a filleted box's top outline chamfered from its long side): OCCT fails only at the chain's far corner, with no faulty contour, so a report read only through contours names nothing; reading the corner instead names bystanders at other corner-only failures
- Two edges of one tangent chain given to a two-distance or distance-and-angle chamfer: the chain takes its reference face from the lower-numbered, so an edge that chamfers alone fails with its chain-mate; judge a named edge by chamfers of it with each other named edge, not alone
- Planes whose axes are left-handed (gp_Ax3 indirect, Du × Dv opposite to `Axis()`): OCCT's chamfer faces, a revolve's bottom face, and anything mirrored have them. Code that takes `gp_Pln::Axis()` for the normal and flips it only for REVERSED faces gets those faces facing in; check normals with Σ A·n = 0 and V = ⅓ Σ A (n · c) over a flat solid
- Booleans where one solid's edge lies in the other's face (a prism's face through a box's edge): OCCT's cut keeps the edge as an INTERNAL edge of the face, which `TopExp::Vertices(edge, f, l, true)` gives no vertices for, and `TopExp::MapShapes` keeps the orientation it first met
- Mesh checks that trust OCCT's own deviation figure (`Poly_Triangulation::Deflection`, a centroid sample in the face's parameters): on cones it overstates by 10-1000x (a 30 mm, 1 mm tall cone says 0.128 mm at 0.1 mm, its mesh 0.001 mm off), so countersinks and rim chamfers are refused; measure each primitive's mesh against an analytic distance function instead
- Real parts' cones (NIST CTC 01, FTC 06): OCCT meshes them with two nodes at exactly the same point inside one face, so a weld through B-rep edges and vertices alone leaves triangles of no area that FreeCAD's coordinate merge drops; export the corpus at the default tolerance
- Fine mesh tolerances where an angular rule takes over (FreeCAD's min(0.1, 5t + 0.005) rad is 0.01 at 1e-3 mm whatever the size): a 1 mm sphere is 805,418 triangles and three minutes of OCCT, past a worker's time limit; time each primitive across the tolerance range

## Feature tree and naming

- References to a deleted feature, a deleted sketch edge, or a face that an edit removed
- Reordering features to create a dependency cycle; inserting a feature before something it references
- Deleting a feature in the middle of the tree
- Two features with the same display name
- Features that produce zero faces or an empty result
- Long undo/redo sequences mixed with recomputes
- References by stale names after an upstream edit
- Ambiguous (`~n`) names that the next feature keeps unchanged: a "duplicated" set filled only by one map's own collisions forgets them, so the pair is unmarked downstream; check `is_duplicated` one feature later
- Element number 0 in a name (`k7f2qx3m.Face0`): a map that answers for any number of a kind resolves it, and a bound of `index <= count` lets it through; elements count from 1
- Counters that count from 1 and print a 1 as nothing (positions, `nth`, `G2` vs `G`): 0 prints as 1 does, so a name's text no longer says which name it is; compare `Display` of every constructible value
- A long timeline of disjoint fuses (bolts fused one at a time): every feature's element map holds all its solid's names and all are rebuilt each publish, so time and memory grow with the square of the timeline
- Renumber a result's elements (permute faces, edges, vertices consistently through the history and topology): names that aren't duplicates or bare numbers must not change
- Primitives whose numbering follows their values (a cone whose top radius goes to 0: OCCT drops the top cap and numbers the base Face2, the top's number): a name derived from `cone.Face2` (a hole's top rim) finds the base rim; check where a fillet of it takes material, before and after
- Elements a boolean keeps whole (a pin wholly inside a block: the void's rim is the pin's circle) under the boolean retyped (cut to common) or its operands swapped: a rule about which side an operation keeps that's applied only to modified elements lets the kept one keep its bare name in both solids, and a fillet of the void's concave rim silently rounds the pin's convex one; check where the fillet takes material
- Examples an error message gives ("as `m3tq8v0z.cut.Edge1`"): feed each back through the same gate; an example in a form the gate refuses (an operation's element by its number) sends an agent from one refusal to another
- Advice that names an element as a feature usually makes it (a sweep's start edge, `M`) given whatever the extent: a symmetric or reversed through pocket starts outside its base and keeps no start edge; follow the advice on every extent

## Files

- Empty, truncated (cut at every byte offset), wrong version, future version
- Unknown keys, missing required keys, duplicate IDs, dangling references, cyclic dependencies
- Unicode in names: right-to-left text, emoji, combining characters, null bytes, 10 MB names
- CRLF line endings, a byte-order mark, invalid UTF-8
- Very deep nesting, 100 MB documents
- External references that use path traversal (`../../etc/passwd`) or point to missing files
- B-reps whose locations mirror or scale (OCCT forbids setting them since 7.6, but BinTools reads them): rewrite the 12 doubles of a `translate`'s location in `to_brep`'s bytes and fix the CRC. A mirrored box reads as valid with volume -6000, and its mesh's normals and triangles disagree
- Values the in-memory type holds but the reader refuses (an empty name, a feature version other than 1): `write` saves a file that won't open; test `read(write(d)) == d` over every constructible value, not only generated valid ones
- Version numbers past the reading type (2^32 + 1 read into a u32 wraps to 1, FreeCAD 913e409b24; 2^64 fails u64 parsing and is called "not a whole number"): refuse as an unknown version
- Errors that quote the input: a 1 MB number, key, or string echoed into the message, parameter, and suggestion makes a multi-MB error; keys holding ESC sequences or newlines reach terminals and logs raw, and an empty key reads as nothing
- References quoted whole into an error's `elements`: the name grammar takes five sources at seven levels, 1.3 MB of one name, and each broken one's error carries it again
- Invisible characters where whitespace may go (U+00A0, U+200B, U+FEFF after the start, U+2028): errors that print the character itself show ` ` or nothing; check they name it by code point
- Memory amplification of a parsed tree: a 100 MB array of `0` under an unknown key, measured with `/usr/bin/time -l` (a tree of nodes and a `Vec<char>` cost 33x here), and no size cap to refuse it with
- Many features each naming an element of one large entity (20,000 revolves whose `axis` a 10,000-point sketch hasn't): a gate that finds each by scanning the entity takes features × elements; time it against the same features naming a built-in (`x_axis`)
- Schemas generated from Rust integers (schemars writes `"format": "uint32"`, `"minimum": 0`, no `"maximum"`; formats are annotations): the schema takes 4294967296 where the reader refuses it
- What the core adds to a document on save (fingerprints, metadata) when it opened just under the size limit: it opens, but can't be saved again
- Error advice written before a new feature kind existed: a sketch of 30,000 points at `5e-324` (6 bytes each in the file, 326 written in full) is too large to save, and is told to shorten its name or note
- Several bad values in one entity at once (a box with length, width, and height all 0) given to a checker that promises every problem: a rule returning `Result` stops at the first, so the user fixes one value per round trip
- Lists only the document's size bounds, checked for repeats by `list[..i].contains(x)`: 40,000 names take 15 s in the gate, a 16 MB document's 600,000 about an hour
- Values nested in an object (`size.distance`) refused by a helper that names its own key: the reader says `distance`, the gate and schema `size.distance`
- Names and notes shown to people (an outline, a diff view) with every bidi control, not only U+202A..202E and U+200E/F: the isolates U+2066..2069 and U+061C slip past most hand lists, and General_Category=Cf (soft hyphen, tag characters U+E0020..E007F) prints as nothing
- Size limits a gate counts against what the translation really sends (the origin's 2 values, while the axes' ends add 4 more and 3 pieces of geometry): a sketch at the gate's limit passes, and the worker refuses it

## Command line

- Arguments that aren't UTF-8 (`caf\xe9.silico`): `std::env::args()` panics; Linux file systems allow such names (APFS refuses them, but the argument still panics)
- `--` and file names that start with `--`, `-`, or a space: a parser that takes every `--x` as an option can't name some files
- A document named as a FIFO with no writer, `/dev/zero`, or a FIFO fed past the reader's limit: `std::fs::read` blocks forever or reads without bound before the reader's size check runs
- A rewrite by rename-over (write beside, then rename): a symlink becomes a regular file and its target is never written, a hard link is split, and a 0444 file is rewritten since rename needs only the directory writable
- A file edited while a command that rewrites it computes (a slow worker wrapper script: `sleep 3; exec worker "$@"`): the edit saved meanwhile is overwritten without a word
- Environment failures (a worker that exits at once, or is missing) through each command and `--json`: they must be "couldn't run" (exit 2), not problems with the document, and every file named needs a result line, even one that couldn't be read

## MCP and agent input

- Malformed JSON, wrong types, missing fields, unexpected extra fields, out-of-range enum values
- Huge strings and arrays
- Many concurrent calls, calls made during recompute, cancellation partway through an operation
- Element references by stale, misspelled, or made-up names
- Calls in the wrong order (editing before opening a document, committing a transaction that was never started)
- One call wrong in many ways at once (a field of every kind wrong, a list of 150 bad members, a kind that makes the others meaningless, a bad limit with a feature that's a sketch): check every reason is given, the first hundred listed and all counted, none hidden behind an early return or behind the one checked first
- Listings with cursors and limits: a cursor of another feature or naming nothing, the last entry's, pages of 1; a "never broken" tie exactly ending at the most a call returns and one past it, at the limit and one off it, ascending and descending

## Kernel worker and IPC

- Worker killed with `kill -9` partway through an operation
- Worker returning garbage, a truncated message, or a reply to a different request
- Slow worker (timeout path); worker that never replies
- Operations that use up memory (a pattern with 1e6 instances, a huge mesh export)
- Many concurrent requests, worker restart during a burst of requests
- A result that arrives after the request was superseded
- Workers started from several threads at once (a pool's start, replacements after a burst of crashes): on macOS std marks new pipes and socket pairs close-on-exec only after making them, so a sibling forked in between inherits them; `lsof` each worker, and check `Command::spawn` never blocks on an exec-status pipe a sibling holds
- Shutdown landing while a slot replaces a dead worker for a request it has already taken: the new worker, started after shutdown killed every registered one, runs the request to the end
- A worker that answers once more than asked: the left-over reply waits on the socket and fails the next request, someone else's
- Worker executables named by a relative path, or deleted, rebuilt, or moved under a running pool: every replacement re-resolves the path
- Worker counts of `usize::MAX`: a `vec![_; n]` sized before the count is checked panics with capacity overflow
- A worker stopped with SIGSTOP: alive to `try_wait` but silent; its request must time out, and the stopped process be killed and reaped
- A worker that says it started, then closes its socket and lingers (alive to `try_wait`): idle checks and pre-request checks that look only at the process miss it, and a crash report's grace for it to exit overruns the call's timeout
- A worker that says it started, then exits on its own while idle: back-off keyed on "started" relaunches it every idle tick forever
- The app at its descriptor limit (`setrlimit(RLIMIT_NOFILE, open + k)` for k = 0..16) while starting a worker: each k fails at a different step; check code, suggestion, and that no process or descriptor is left
- Executables that can't be started every way: missing, a directory, mode 0644, a script whose `#!` interpreter is missing, an empty path, and the same named through an environment variable; each error must say what to fix
- A worker held at launch: macOS holds a newly written executable at `_dyld_start` while it assesses it, at times past a minute, so the first request times out blaming itself (in tests, link worker binaries rather than copy them)
- A fallback step appended to a chain of attempts: time the whole chain on one failing component at a fraction of the size limit (an impossible chain of 1,024 unknowns: DogLeg 11 s, LM 29 s, BFGS 0.2 s, anchored SQP 33 s, past a 60 s request), not each step alone
- Worker refusals passed up as they came (a value index, `kernel_failed`, no feature): check the core adds the feature and maps indices to the elements it knows
- Log sinks that misbehave each way, as a global `tracing` layer under a shell-script worker that writes chosen log bytes: stalled on events, stalled in `on_close` (a synchronous span exporter), panicking, merely slower than the reader (a `fmt` layer to a file, in a 20,000-line burst); check every line is given or counted, counts are said promptly, crash reports still wait for a log a worker's child holds open, and no lock is held while sink code runs
- A pool that grows on demand, fed requests strictly one after another (thousands of submit-then-wait, and every other one crashing its worker): a slot that counts itself busy until after it has delivered, or while it replaces its worker, makes the next submit start another slot and worker
- A worker executable replaced by a FIFO, or a symlink to `/dev/zero`, under a running pool or as its worker starts: whatever hashes or reads the executable blocks opening the FIFO (a slot thread, a reply thread, a pool drop that joins them) or reads without end
- An executable rewritten in place with another build of the same size, its modification time put back (`cp -p`, `rsync --inplace -t`): a size, time, and inode stamp calls it unchanged; only its change time (`st_ctime`) shows it
- A worker found untrusted (a start check failed) and then given a long request: time the call; a refusal read only once the reply comes waits out the request
- Error codes decided by a fact (said it started or not) that an earlier check pre-empts (a solve that finds its worker already exited and calls it a crash): the same binary that exits at launch gets two codes by timing; call it at once and 300 ms late, and compare
- A code split on one fact while its advice is still keyed on another (started or not, against how it ended): a kill from outside before the first frame is "no retry helps" by its code and "try the request again" by its advice
- A launcher that backgrounds a child (holding the socket) and exits before its first frame: the call waits out its whole timeout, reported as a hang, not as the exit it was

## Recompute and scheduling

- Generation counters at their end (a kernel already at `u64::MAX`): a `+ 1` panics (or wraps to 0 in release, so every request is superseded), and a `saturating_add` leaves the generation where it was, so a cancel cancels nothing
- A panic under one of several threads that count themselves running under a shared condvar (a kernel bug in `submit`, say): the others wait for it forever, the scope never joins, and the handling for a dead thread is never reached; wrap a test kernel that panics on one op
- A naive sequential evaluator as the oracle: random timelines with value-based refusals (kept and not kept), several edits in a row, comparing every feature's outcome, the bodies, which keys are equal, and exactly which steps each edit ran
- Results taken from the cache or memory without their inputs' (an entry lost, a subdirectory that's a link): a step that reads more than its inputs (a fillet naming its base's elements from the whole lineage) runs before they're rebuilt
- A superseded recompute that ends after the newer one: both keep their results by key, the last wins, and the older one's copy (its measure superseded, so unmeasured) overwrites the newer's measured or refused one; hold the older one's requests, start the newer, and loop a few hundred rounds, since which keeps last is a race
- A measure made of several requests (a summary, then batches of a description): refuse, crash, supersede, or cancel at a later batch after the earlier ones came back; nothing partial may be published, kept in memory, or written, and the retry starts from the first request
- Features alike in every value but their IDs (two identical boxes each pocketed alike, or a refused feature deleted and re-added under a new ID): the deduplicated step, or the refusal kept by its key, carries the first feature's words, so the second is told another feature and base failed

## Oracles and comparisons

- Pairs of different shapes with equal volume, area, bounding box, and counts (a two-distance chamfer and its flip, a frustum upside down, a fillet on the opposite parallel edge, a feature below the relative tolerance): a comparison of global measures passes either
- The reference program's own settings: FreeCAD reads (and rewrites) the GUI's user.cfg, and PartDesign's `Refine` follows it; pin every property a call reads, and isolate the config with `FREECAD_USER_HOME`
- Element numbers turned into Python indices: 0 becomes −1, the last element (`Edges[i - 1]`)
- Measures that depend on parametrization, not geometry (OCCT's fixed-order integration, BRepBndLib's loose box): two builds of one OCCT differ beyond a tight tolerance on curved booleans; sweep thousands of seeds before calibrating
- A reference process that hangs (no deadline), and a crash partway through a batch on either side: later cases must still run in a fresh process
- No cases at all (an empty directory, `--count 0`): a harness that compared nothing must not exit 0

## Import

- Malformed STEP, STL, or 3MF: truncated files, wrong units, inverted normals, non-manifold meshes, zero-area triangles
- Valid STEP files with features OCCT handles badly: very large assemblies, tiny sliver faces, self-intersecting surfaces

## Fakes and test doubles

- Inputs one ULP inside a check's edge that the real component decides by computed geometry and the fake by `== 0` (a cone's small radius of 1.0000000000000002e-7 under a radius of 1: OCCT drops the cap and makes an apex, 2 faces; the fake says 3): run the same edge-biased cases through both and compare
- The same shape spelled two ways (a radius or offset of -0.0 against 0.0, a move by nothing, which the real kernel returns as its input): the real kernel gives one set of bytes, a fake keyed on the values' text gives several
- A result decided under a lock but sent after it's released: an `advance` landing between the two returns with nothing delivered, and the superseded generation's result arrives after it; sweep the advance across the request with a spin loop, with threads waiting on the condvar the finisher signals to widen the window
- Callbacks a test hands the fake (hold and refusal rules) that panic: the serving thread dies with the lock held and its request counted as running, its caller is told the kernel stopped, and every later request hangs
- Waits of "forever" (`Duration::MAX`) added to `Instant::now()` unchecked, and sizes of `usize::MAX` collected into a Vec: panics where the real component returns an error
- IDs parsed with `from_str_radix`, which takes upper-case digits and a leading '+': bytes the component never made are taken as one of its own
- A fake's step read by its grammar but not its values (a fillet of edge 13 of a box, a radius of -1, an edge listed twice): bytes no request could make are taken as its own
- Hand-written topology tables (each face's edges, each edge's vertices) for primitives: compare with the real kernel's move history, cones whose cap OCCT drops included

## Caches and files the core writes

- An entry replaced by a FIFO, or a symlink to one or to `/dev/zero`: `std::fs::read` blocks in open(2) forever or reads until memory runs out; a non-regular file must be a miss
- Partial or temporary files at predictable names (`.partial-<pid>-<n>`) planted beforehand as symlinks: `File::create` follows them and overwrites the link's target outside the cache; check a victim file is untouched
- A sound entry re-encoded with an inconsistent payload and its checksum fixed (a checksum isn't a signature): whatever the core validates from a worker (a history's inputs and element numbers) must be validated when read back from disk
- Decisions the core makes before writing, read back unchecked: an entry keeping a refusal of a code the core never keeps (a hang's), a summary no reply could give (a NaN volume), an error naming more elements than its total; each is never asked again
- Parts of one entry that must agree, each sound alone (a description's element counts and its summary's, another solid's description under this one's key, 100,000 sound vertices for a box): checking each part's values but not one against another publishes a model whose parts disagree
- Keys hashing parts joined by a separator the parts may contain (a newline in a kernel identity or platform): two different keys hash the same bytes
- A kernel identity that hashes only the executable: libraries it loads at run time (OCCT's dylibs through an rpath symlink) change results without changing the identity; a worker binary replaced under a running pool is relaunched under the old identity
- Directory sizes summed from file lengths: sparse files cost no disk, so a few hundred planted at the file system's longest (APFS: 2^55 bytes) sum past `u64::MAX`; a plain `+=` panics in debug and wraps in release
- Bounds kept by "scan, evict, then write" across threads: entries other threads are writing aren't on disk to evict, so N puts at once leave N entries; bound every entry above half the bound and run parallel leaves
