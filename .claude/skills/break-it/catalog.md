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
- Mesh normals at a surface's singular points (a cone's apex, a sphere's pole, a horn torus's center, a fillet's corner ball): check each vertex normal is on the outward side of every triangle it's in. Skip slivers by aspect, not absolute area: three collinear nodes 1e5 from the origin have a cross product of 1e-11 pointing anywhere
- Fillet or chamfer an edge that ends where two solids touch at a single vertex (two cubes fused corner to corner): OCCT segfaults, as FreeCAD does
- Edges whose faces meet at a few degrees (a 3° crease, every side edge of a regular 63-gon or finer prism): OCCT calls normals within 0.1 rad tangent and won't fillet or chamfer them; check the error doesn't misdescribe the corner
- Sizes too small to cut (a distance-and-angle chamfer at 1e-9°) or a size OCCT fails at whatever it is: check the error's suggestion doesn't send the user the wrong way ("try a smaller size")
- Angles at a singularity: a distance-and-angle chamfer at exactly 90° on an acute edge gives an invalid solid one way round, while 89.99° and 90.01° are fine
- The same edges listed in another order: OCCT's chamfer of all 12 box edges at just under half the face builds listed 1 to 12 and fails listed 12 to 1, and a tangent chain takes its reference face from the first edge listed
- Fillets that collide only in pairs (two parallel edges on a face narrower than 2r) with a bystander edge listed before them: OCCT's faulty-contour report (`ChFi3d_StripeEdgeInter` in `ChFi3d_Builder::Compute`) names only the first contour of the colliding pair, which can be the bystander; check every edge a refusal names against a hand calculation of whose r × r strips overlap, and follow the "leave it out" advice to see what it costs
- Chamfers whose tangent chain runs past a face away from the edge given (a filleted box's top outline chamfered from its long side): OCCT fails only at the chain's far corner, with no faulty contour, so a report read only through contours names nothing; reading the corner instead names bystanders at other corner-only failures
- Two edges of one tangent chain given to a two-distance or distance-and-angle chamfer: the chain takes its reference face from the lower-numbered, so an edge that chamfers alone fails with its chain-mate; judge a named edge by chamfers of it with each other named edge, not alone

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

## Recompute and scheduling

- Generation counters at their end (a kernel already at `u64::MAX`): a `+ 1` panics (or wraps to 0 in release, so every request is superseded), and a `saturating_add` leaves the generation where it was, so a cancel cancels nothing
- A panic under one of several threads that count themselves running under a shared condvar (a kernel bug in `submit`, say): the others wait for it forever, the scope never joins, and the handling for a dead thread is never reached; wrap a test kernel that panics on one op
- A naive sequential evaluator as the oracle: random timelines with value-based refusals (kept and not kept), several edits in a row, comparing every feature's outcome, the bodies, which keys are equal, and exactly which steps each edit ran
- Results taken from the cache or memory without their inputs' (an entry lost, a subdirectory that's a link): a step that reads more than its inputs (a fillet naming its base's elements from the whole lineage) runs before they're rebuilt

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
- Keys hashing parts joined by a separator the parts may contain (a newline in a kernel identity or platform): two different keys hash the same bytes
- A kernel identity that hashes only the executable: libraries it loads at run time (OCCT's dylibs through an rpath symlink) change results without changing the identity; a worker binary replaced under a running pool is relaunched under the old identity
