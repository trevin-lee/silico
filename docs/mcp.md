# Silico over MCP

`silico serve` is Silico's MCP server. An agent connected to it opens `.silico` documents, edits their features, reads the model's elements and measures, renders the part, and exports it, with each command one tool. This guide covers connecting an agent and the workflow it follows. The shapes are in [design/m4.1-command-api.md](design/m4.1-command-api.md) (every command's request, result, and errors) and [design/m4.9-mcp.md](design/m4.9-mcp.md) (the server: protocol, exact JSON, cancellation, progress); the CLI's `serve` is in [cli.md](cli.md).

## Setup

**Build.** From the repository:

```bash
pixi run build      # OCCT first if its inputs changed, then the workspace
```

This puts `silico` and its workers (`silico-kernel-worker`, `silico-sketch-worker`, `silico-render-worker`, `silico-step-worker`) in `target/debug/`. `silico` looks for each worker beside its own executable, following any links to it, not in the working directory or on `PATH`. So the client can run `target/debug/silico` or a symlink to it. A copy elsewhere finds no workers until each is beside it or named by `SILICO_KERNEL_WORKER`, `SILICO_SKETCH_WORKER`, `SILICO_RENDER_WORKER`, or `SILICO_STEP_WORKER`.

`silico serve` checks its four workers (kernel, sketch, render, STEP) when it starts, and starts with all of them or not at all. If any is missing it doesn't start: it exits 2 before the handshake, and stderr names each worker missing, the directory it looked in, and the worker it looked for.

**Connect Claude Code.** One server per client, started by the client over stdio:

```bash
claude mcp add silico -- /path/to/silico/target/debug/silico serve --workspace /path/to/models
```

This adds it for the directory you run it in. For every project use `claude mcp add --scope user silico -- …`; to share it through the project's `.mcp.json`, use `--scope project`. Options go before the name, never after `--`. Set the server's environment the same way, with another option between `--env` and the name: `claude mcp add --env SILICO_LOG=debug --scope user silico -- …`. Run `/mcp` in Claude Code to see that it connected. Any other MCP client takes the same command in its config, with `"env": {...}` beside `args` for its environment:

```json
{
  "mcpServers": {
    "silico": {
      "command": "/path/to/silico/target/debug/silico",
      "args": ["serve", "--workspace", "/path/to/models"]
    }
  }
}
```

**The workspace** is the directory the documents are in: `--workspace DIR`, or the directory the client starts the server in if it's left out. Give it, since clients differ in where they start servers. Every path a tool takes (`open`'s, `render`'s `to`, `export`'s) is relative to it, and one leading out of it, by `..` or through a link, is refused. The directory must exist: a workspace that can't be reached, or the root of a disk, stops the server at startup (exit 2), with the reason on stderr.

**Where things live:**

- **Sessions** are in the server's memory: `s1`, `s2`, ... in the order `open` made them, at most 16 open at once. They end with the server. A committed edit is in the file; an open transaction is rolled back and its edits are lost.
- **Documents** are the `.silico` files in the workspace, written only by `open` with `create`, by `commit`, by `format`, or by an edit made outside a transaction.
- **The rebuild cache** is `.silico-cache/` beside each document, ignoring itself in git, its entries sealed with a secret of yours at `~/.cache/silico/cache-secret` (or `$XDG_CACHE_HOME/silico/cache-secret`). `cache_clear` empties it.
- **Logs** go to stderr, where the client keeps its MCP servers' logs: `warn` and above, unless `SILICO_LOG` (a `tracing` filter, set in the client's config) says otherwise. Stdout holds only MCP's messages. Each session also keeps its own recent events for `read_logs`.

`tools/list` gives 26 tools, each tool's `inputSchema` its request and `outputSchema` its whole answer. Its input schemas are about 55 KB of an agent's context (docs/design/m4.9-mcp.md, *Size*).

## The workflow

The server's `instructions`, which every client is given at `initialize`, say the three rules an agent most needs: edit inside a transaction, read `model: "current"` after an edit, and name an element only as `elements` or `query` gives it.

**Every argument is given.** A request is read strictly: each field its shape has must be present, `null` where it allows none (`"feature": null` for every body), and a field it doesn't have is refused. `schema` (`{"of": "query"}`, or `{"of": "document"}` for the document's JSON Schema) gives the same shapes `tools/list` does.

| Step | Tools |
|---|---|
| Open a document, or make one | `open` (`path`, `create`). A document the gate refuses still opens, with its problems in the result, so it can be mended with `replace_text`; the other edits are refused until it is. |
| See what's there | `outline`, `text`, `status`, `check`, and `timeline` (the features in order, structured: each one's type, name, inputs, and health from the model named, and the bodies) |
| Find elements by name | `elements` (a page of a solid's faces, edges, and vertices, each with its geometry and measures; `after` for the next page) and `query` (a kind, a surface or curve type, `facing` a direction, ranges of measures, an order, a limit). Use a name exactly as given. A feature that stores names (a fillet's or chamfer's `edges`) takes only stable ones. An element whose `name` is unstable (one that came about for more than one element, a `~n` duplicate, or an element by its number) may have a stable name in `aliases`; use that, and if it has none, no stored reference can name it. `distance`, `angle`, and `render`'s `highlight` take any name. A stored name is found again after upstream edits, or fails as `reference_broken` with candidates, never re-pointed. |
| Edit | `begin`, then `add_feature` (`before` a feature, or `null` for the end), `set_field`, `remove_feature`, `move_feature` (`before` a feature, or `null` for the end), `replace_text`, `sketch_add`, `sketch_remove`, `sketch_split`, `sketch_trim`, `sketch_unjoin`, `sketch_merge`, or `sketch_extend`, each with the `revision` the session's last answer gave. Inserting, moving, and removing rewire the chain of `base` links they change, each change listed in `rewired`: a removed feature's consumer takes its base, and one refused names both features where nothing can stand in. Then `commit` to save or `rollback` to undo every edit since `begin`. An edit outside a transaction saves at once. Outside a transaction, a change made to the file by someone else is reloaded before the next command (`"reloaded": true`, a new revision), and an edit at the old revision is refused as `conflict`. Inside one, `commit` is refused as `conflict` if the file changed. |
| Edit a sketch | `sketch_add`: points, lines, circles, arcs, and constraints, each as the document writes it with a fresh random 8-character ID, never one the sketch holds or a removal freed (a name a feature stores of the removed member would find the new one), checked whole, each problem at its member's path (`circles.cb000003.radius`). `sketch_remove`: members by ID; a curve takes the points no other curve uses and every constraint on what goes, and a point a curve still uses is refused. Neither is refused for what the sketch's solve then finds: a constraint that over-defines it is added, and `check` reports the conflict (make the dimension driven with `set_field`, or remove it). `sketch_split` (a line or an arc at one position on it, a circle at two), `sketch_trim` (the part of a curve between the curves that cut it either side of `pick`), `sketch_unjoin` (one curve off a point it shares), `sketch_merge` (points made one, the first named kept where it is), and `sketch_extend` (a line's or an arc's `start` or `end`, `by` a length, an angle, to a point on its carrier, or to the `next_curve` it meets, which the core finds; nothing holds it there after, so merge the end into a point it lands on) write geometry, so the sketch's solve judges each against the solve before it: one whose solve is worse (a conflict or a redundancy it adds, or no solution) is refused, the sketch as it was, but a conflict the sketch had already isn't the edit's, though a line's end moves only in a sketch whose constraints agree: in one that conflicts `sketch_extend` refuses it `over_constrained`, naming the conflict, as a drag is (for a merge that fails, name first the point the constraints hold). Their `ids` give the new members' IDs, in the order the result's `added` lists them, or `null` for the core's; a replay passes back the `added` it recorded. A split or trim lists in `references` the features whose stored names its curve reached: each now finds only the start-side piece, so read them again (`elements`). |
| Read the model | `check`, `elements`, `query`, `mass_properties`, `distance`, `angle`, `render`, `sketch` each take `model`: `current` waits for the session's latest edit to rebuild; `published` answers at once from the newest model rebuilt, which may be older. The answer's `model.revision` says which it read. |
| Measure | `mass_properties` (volume, area, centre of mass), `distance` and `angle` (between two named elements) |
| Read a sketch | `sketch`: a sketch as the model solved it, its plane in 3D, its state (`under_defined`, `fully_defined`, `over_defined`, `no_solution`, `invalid`) and degrees of freedom, each point, curve, and constraint as the document writes it at its solved values with its own state (a constraint `satisfied`, `conflicting`, `redundant`, `partially_redundant`, or `unsolved`), its profile's regions by curve ID or why it makes none (`profile_problems`), and why it wasn't solved (`solve_problem`). |
| See it | `render`: a named `view` or a `camera`, `width` and `height`, elements to `highlight`, and `to`, a PNG path in the workspace or `null`. The PNG comes back as an image beside the result. |
| Export | `export`: `format` `3mf` with a `tolerance` in mm (FreeCAD's is 0.1), or `step` with `tolerance: null`; `path`; `replace`; `features` (`null` for every body). It reads the current model. |
| Diagnose | `read_logs`: the session's recent events of a `level` and every level more severe, of one `feature` or all, `after` the last read's `next`, at most `limit`. It answers at once, even while a rebuild runs. |
| Long work | A call whose `_meta` has a `progressToken` gets MCP progress notifications while it rebuilds. MCP's `notifications/cancelled`, or the `cancel` tool, stops a call. `status` says what's running. |
| Finish | `close`. It's refused while a transaction is open. |

Feature IDs: `add_feature`'s feature may give its `id`, or leave it out, and the core makes one. Sketch members (points, lines, circles, arcs, constraints) always give theirs, unique within the sketch. An ID is 8 lower-case Crockford base32 characters (digits and letters but i, l, o, and u), such as `s0000001` or `pa000001`.

**Keep results small.** Each element entry is about 600 bytes, and each result is sent twice (as `structuredContent` and as text). Claude Code cuts off a tool's result past 25,000 tokens by default (`MAX_MCP_OUTPUT_TOKENS`). So ask `elements` and `query` for small pages (a `limit` of 50 or so), narrow `query` by kind, type, and measures, and read one feature's text with `text`'s `feature`.

## Errors and refusals

Every tool answers with its command's envelope, as `structuredContent` and as the same text in `content`. A command that ran has `ok: true` and its `result`. One that was refused or couldn't run has `ok: false` (and MCP's `isError: true`), with every reason in `errors`, the first 100 listed and `total` counting them all. For example, a second `begin`:

```json
{"ok": false, "session": {"id": "s1", "path": "part.silico", "revision": 1, "transaction": true, "reloaded": false}, "model": null,
 "errors": [{"code": "invalid_input", "message": "a transaction is already open in this session", "feature": null, "elements": [], "elements_total": 0, "parameter": "session", "cause": null, "suggestion": "commit or roll back the open transaction, then begin again", "candidates": []}],
 "total": 1}
```

Each error says:

- `code`: one of [the codes](../crates/silico-core/tests/contract/error-codes.txt), never renamed;
- `message`;
- the `feature` and the `elements` at fault (the first 100, with `elements_total`);
- the request's `parameter` at fault;
- the underlying `cause`;
- a `suggestion` of what to do;
- for a name that finds nothing, `candidates`: the likeliest names now, none of them taken for it.

```json
{"code": "reference_broken", "message": "elements.1, \"k7f2qx3m.box.Face7\", names no element of the live solids", "feature": null, "elements": ["k7f2qx3m.box.Face7"], "elements_total": 1, "parameter": "elements.1", "cause": null, "suggestion": "name the element as the model now names it: the candidates are the likeliest, none taken for it", "candidates": ["k7f2qx3m.box.Face6"]}
```

A document's problems are not a refusal: a `check` that finds a failed fillet has `ok: true`, with the problems and warnings in its result. `worker_crashed`, `worker_timeout`, `worker_unavailable`, `protocol_error`, `resource_exhausted`, and `internal` are kernel faults, not the document's. A rebuild that met one says so once, in `model.fault`, and its revision keeps it, so reading again returns the same fault. Follow the suggestion, which is usually `check` with `fresh: true` to try it again.

A tool that doesn't exist, or arguments that aren't an object, is a JSON-RPC error (`-32602`) rather than a result, its `data` the same refused envelope. An unknown tool's lists the tools as candidates.

**Limits.** `version` returns every limit as data, and [the design's *Limits*](design/m4.1-command-api.md#limits) says what happens past each. Most are refused as `invalid_input`, with the limit in the message. The ones an agent meets first: `elements` and `query` return at most 1,000 entries a call, at most 16 sessions are open at once, and a kernel request times out after 60 s, as `worker_timeout`, a kernel fault. No command has a limit on its whole time. The server runs at most 64 calls at once, and refuses a call past that, saying so.

## A worked example: the bracket

The recorded session that M4's done test replays, [crates/silico-cli/tests/bracket/session.jsonl](../crates/silico-cli/tests/bracket/session.jsonl), designs the L-bracket of [its spec](../crates/silico-cli/tests/bracket/spec.md): two legs, 60 and 50 long, 40 wide, 5 thick; two 5.5 mm holes through each leg; the inside corner filleted at radius 5. Below are most of its calls (`tools/call`'s `name` and `arguments`) and the parts of their replies the agent acts on, abbreviated with `…`. [The M4 walkthrough](walkthrough/m4.md) goes through it step by step, with the values worked out by hand.

```
open        {"path": "bracket.silico", "create": true}
         →  session s1, revision 1; result {"created": true, "features": 0, …}
begin       {"session": "s1"}                                       → {"base": 1}
add_feature {"session": "s1", "revision": 1, "before": null, "feature": {"id": "s0000001", "type": "sketch",
             "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0,
             "construction": false}, …], "lines": […], "circles": [], "arcs": [],
             "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, …,
             {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, …]}}
         →  revision 2
add_feature {"session": "s1", "revision": 2, "before": null, "feature": {"id": "p0000001", "type": "pad", "name": "Legs",
             "sketch": "s0000001", "extent": {"type": "one_side", "length": 4, "reversed": true}}}
         →  revision 3
check       {"session": "s1", "model": "current", "fresh": false}   → {"problems": [], …}
mass_properties {"session": "s1", "model": "current", "feature": null}
         →  {"bodies": [{"feature": "p0000001", "volume": 2100, …}]}
```

The spec's legs are 21,000 mm³, not 2,100: the pad is 4 deep where it should be 40. So the agent undoes the edit.

```
rollback    {"session": "s1"}                                       → {"restored": 1}, revision 4
begin       {"session": "s1"}                                       → {"base": 4}
add_feature …the same sketch, at revision 4; then the pad, "length": 40, at revision 5
mass_properties {"session": "s1", "model": "current", "feature": null}
         →  {"bodies": [{"feature": "p0000001", "volume": 21000, "area": 9850, …}]}
add_feature …a sketch of two circles on "xy" (s0000002), then
            {"id": "h0000001", "type": "pocket", "name": "Base holes", "base": "p0000001",
             "sketch": "s0000002", "extent": {"type": "through_all", "reversed": true}}
add_feature …the same through the upright, on "yz" (s0000003, h0000002), to revision 10
mass_properties {"session": "s1", "model": "current", "feature": null}
         →  {"bodies": [{"feature": "h0000002", "volume": 20524.834111, …}]}
query       {"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "line",
             "stable": true, "facing": null, "measures": [{"measure": "length", "at_least": 40, "at_most": 40}],
             "order": null, "limit": 10}
         →  {"elements": [{"name": "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000001)", …}, …,
             {"name": "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)", "stable": true,
             "start": [5, 0, 5], "end": [5, 40, 5], …}, …], "matched": 6, …}
```

Of the six edges, the one from (5, 0, 5) to (5, 40, 5) is the inside corner. The agent fillets it by the name `query` gave.

```
add_feature {"session": "s1", "revision": 10, "before": null, "feature": {"id": "f0000001", "type": "fillet",
             "name": "Inside corner", "base": "h0000002",
             "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "radius": 5}}
commit      {"session": "s1"}
         →  {"written": true, "path": "bracket.silico",
             "refingerprinted": ["p0000001", "h0000001", "h0000002", "f0000001"], "problems": [], …}
check       {"session": "s1", "model": "current", "fresh": false}
         →  {"problems": [], "problems_total": 0, "warnings": [], "warnings_total": 0}
mass_properties {"session": "s1", "model": "current", "feature": null}
         →  {"bodies": [{"feature": "f0000001", "volume": 20739.435948, "area": 9930.398194,
             "center_of_mass": [18.019555, 20, 13.014015]}]}
query       …"kind": "face", "type": "plane", "facing": {"direction": [1, 0, 0], "within": 0},
             "order": {"by": "area", "descending": false}, "limit": 1
         →  {"elements": [{"name": "p0000001.pad.Face(G s0000001.sketch.Edge.na000002)", "area": 200, …}], …}
query       …"kind": "edge", "type": "circle", "measures": [{"measure": "radius", "at_least": 2.75, "at_most": 2.75}], "limit": 8
         →  the eight rims, the first "h0000001.pocket.Edge(G p0000001.pad.Face(G s0000001.sketch.Edge.na000003),
             G h0000001.pocket.Face(G s0000002.sketch.Edge.cb000001))", …
distance    {"session": "s1", "model": "current", "feature": null, "elements": ["h0000001.pocket.Edge(…cb000001))",
             "p0000001.pad.Face(G s0000001.sketch.Edge.na000002)"]}
         →  {"distance": 17.25, "from": [42.75, 12, 5], "to": [60, 12, 5], "center_to_element": 20, …}
render      {"session": "s1", "model": "current", "view": "top_front_right", "camera": null, "width": 800,
             "height": 600, "highlight": [the rim, the face], "to": "bracket.png", "replace": false}
         →  {"png_bytes": …, "path": "bracket.png", "width": 800, "height": 600, "camera": {…}}, and the PNG as an image
export      {"session": "s1", "format": "3mf", "path": "bracket.3mf", "replace": false, "features": null,
             "tolerance": 0.1}
         →  {"path": "bracket.3mf", "bytes": 148501, "objects": [{"feature": "f0000001", "name": "Inside corner",
             "triangles": 2180, "vertices": 1084, "volume": 20739.948255, "left_out": []}]}
close       {"session": "s1"}                                       → {}
```

By hand, the bracket's volume is 22,000 − 401.25π ≈ 20,739.4359 mm³, as `mass_properties` reads it. The 3MF's mesh holds 0.512 mm³ more, because its chords cut across the concave holes and fillet. The hole's centre is 20 from the end face, and its rim 17.25.
