# The `silico` command

Each command is a thin client of the core's commands (`silico_core::command`), which the app and MCP run too, so they all do the same thing.

| Command | What it does |
|---|---|
| `silico check [--json] FILE...` | Reads each document, puts it through the gate, rebuilds it, and compares each feature's stored fingerprint with the rebuild. Every problem goes on stderr. With `--json`, one line per file goes on stdout instead: `{"file", "ok", "problems", "total"}`, where each problem is an error in the core's error contract. A file that couldn't be checked gives `{"file", "ok": false, "error"}`, its error in the same shape (`io_failed` for a file that couldn't be read). Both lines are a contract (`crates/silico-cli/tests/contract/check-line.json`). |
| `silico fmt [--check] FILE...` | Rewrites each document in canonical form with its fingerprints as rebuilt, and says which fingerprints it wrote. With `--check`, writes nothing and says whether it would. A feature that didn't build keeps the fingerprint stored with it, says why, and the file is still written (exit 1). It never writes a file this user may not write (as the system says), one that changed while it was being rebuilt, or one whose rebuild failed for the kernel's own reasons (a worker that crashed or hung: exit 2). It writes through a symbolic link to the file it names, and keeps the file's permissions. A file with other names (hard links) is written in place, its old text kept beside it (`.NAME.fmt-PID`) until the new is on disk: if `fmt` is killed mid-write, that file holds the document as it was. The file is grown to its new length before it's written, so a limit on a file's size stops the write before it changes. That copy needs the directory to be writable, as a rename does: a hard-linked file in a directory this user can't write isn't formatted. |
| `silico show FILE` | The document's outline: one line per feature, with its values and fingerprint, then its note, for a sketch its points, curves, and constraints, a line each as the file writes them, and for a fillet or chamfer its edges, a line each; then the bodies. Needs no kernel. |
| `silico textconv FILE` | Git's text conversion: the outline, if the file reads as a document; otherwise the file as it is (conflict markers, a half-made edit), so `git diff` still shows it. |

`--` ends the options, so a file can be named like one.

**Exit codes:**
- 0: nothing wrong.
- 1: a document has problems, or (with `fmt --check`) isn't in canonical form.
- 2: the command couldn't run. That means a usage error, a file that couldn't be read or written, or a kernel that couldn't start or failed for reasons that aren't the document's (a worker crashed or hung, or something ran short).

## `git diff` of a document

To diff the outline rather than the JSON, add this to the repository's `.gitattributes`:

```
*.silico diff=silico
```

and tell git how to convert (once per clone, or in your global config):

```bash
git config diff.silico.textconv "silico textconv"
```

An edit to one value then diffs as one line of the outline, and each feature's line stays the same unless that feature changes.
