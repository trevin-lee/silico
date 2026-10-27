# STEP export

A document's bodies written as a STEP file (ISO 10303-21) that other CAD systems read as solids: each body a product named by its feature, in millimetres, its bytes the same in every run and on every checked platform. This is M4.8's `format: "step"` (docs/design/m4.1-command-api.md, the `export` entry and decision 6: "its header written from the document so its bytes are deterministic"). It sits beside the 3MF export (docs/ports/mesh-export.md) and shares its command, its path rules, and its `left_out`.

**Built (M4.8, `format: "step"`):** `silico_protocol::step_encoded` (every string, as *Strings* says), `Op::ExportStep` and the STEP worker (`silico-step-worker`, in silico-kernel: the recipe below, OCCT built with `TKDESTEP`, linked into that worker alone), and the `export` command's STEP path (`silico_core::command::export`): each body a product, its id the feature's ID and its name the feature's, under an assembly named by the document's stem if there are two or more, the header's file name the document's. As Fable decided the contract (2026-10-26): `tolerance` is `null` for STEP and a length for 3MF, each mismatch refused naming it; a STEP object's `triangles`, `vertices`, and `volume` are `null`. The worker reports each part's faces left out (INTERNAL and EXTERNAL), anything else the writer's check list failed to transfer, and the faces and face bounds the parts have against those the file holds; the core refuses a failure or a count that differs (`invalid_result`, naming the feature and the elements), never a file with geometry missing. The writer's warnings, which say what it corrected, are logged.

The study before it, and its experiments, follow. The probes ran outside the repository; each, and what it showed, is listed under *Experiments*.

Citations are to FreeCAD 1.1.3 (tag `1.1.3`, commit `145529fe74`; the files fetched from GitHub, since the local clone has no tags) unless marked **main** (`876271d660`). OCCT citations are to 7.9.3 with Silico's patch series, `src/<Package>/<file>:line`.

## What FreeCAD does

### Three paths, one writer

| Path | Code | Writer | Assembly mode | Names |
|---|---|---|---|---|
| `Shape.exportStep(file)`, `Shape.write("x.step")` | `TopoShape::exportStep`, `src/Mod/Part/App/TopoShape.cpp:891-925` | `STEPControl_Writer`, `Transfer(shape, AsIs)` | `write.step.assembly` = Off (`:896`, `c8c4686543`, #6282) | none: OCCT's default product name |
| `Part.export(objects, file)` | `AppPartPy.cpp` `exporter`: every `Part::Feature`'s shape into one `TopoDS_Compound`, then `TopoShape::write` (`TopoShape.cpp:843-866`) | as above | Off | none: one product for all objects |
| `Import.export(objects, file)` and the GUI's File > Export ("STEP with colors", `ImportGui.export`, `src/Mod/Import/InitGui.py:39`) | `src/Mod/Import/App/AppImportPy.cpp:248-366`: an XCAF document filled by `ExportOCAF2` (`src/Mod/Import/App/ExportOCAF2.cpp`), written by `WriterStep::write` (`src/Mod/Import/App/WriterStep.cpp:42-74`) | `STEPCAFControl_Writer`, `Transfer(doc, AsIs)` | `ExportOCAF2` sets Auto (`ExportOCAF2.cpp:76`), then `WriterStep` sets On (`WriterStep.cpp:48`) | each object's label named by its `Label` (`ExportOCAF2.cpp:104-113`); several objects go under an assembly named by the document's internal `Name` (`:345-383`) |

The XCAF path is what a user gets from the GUI. `ImportGui.export` also sets the schema from the preference before writing (`src/Mod/Import/Gui/AppImportGuiPy.cpp`, the `exporter`'s STEP branch). The legacy exporter (`ExportOCAF`, preference `ExportLegacy`) is a fallback only.

### The settings, as `Interface_Static` values

`Part::OCAF::ImportExportSettings::initialize` (`src/Mod/Part/App/OCAF/ImportExportSettings.cpp:40-50`, run when Part loads, `AppPart.cpp:577`) sets the statics from the preferences under `BaseApp/Preferences/Mod/Part`:

| Static | FreeCAD's value | OCCT's default | Where |
|---|---|---|---|
| `write.step.schema` | preference `STEP/Scheme`, default **OCCT's, AP214IS** | AP214IS (`STEPControl_Controller.cxx:93`) | `ImportExportSettings.cpp:150-151` |
| `write.step.unit` | preference `STEP/Unit`: 0 MM (default), 1 M, 2 INCH | MM (`STEPControl_Controller.cxx:163`) | `:137-148` |
| `write.step.product.name` | preference `STEP/Product`, default OCCT's ("Open CASCADE STEP translator 7.9") | "Open CASCADE STEP translator " `OCC_VERSION_STRING` (`STEPControl_Controller.cxx:69-71`) | `:152-155` |
| `write.surfacecurve.mode` (pcurves) | preference `General/WriteSurfaceCurveMode`, default **0: no pcurves** | 1 (`DESTEP_Parameters.hxx`, `WriteSurfaceCurMode = true`) | `:76-77`; FreeCAD's default changed from 1 to 0 in `70c18c8017` (2022) |
| `write.step.assembly` | Off (exportStep), Auto then On (XCAF path) | Auto (`STEPControl_Controller.cxx:77`) | above |
| `write.precision.mode`, `write.precision.val` | not set | Average, 1e-3 (`Interface_StaticStandards.cxx:68-76`) | — |
| everything else (`write.step.vertex.mode`, `write.step.nonmanifold`, `write.step.tessellated`, …) | not set | OCCT's | — |

The XCAF path's own options (`ExportHiddenObject` default true since `70c18c8017`, `ExportKeepPlacement`, the default colour from `View/DefaultShapeColor`) don't change a solid's geometry.

### The header

`exportStep` (`TopoShape.cpp:908-914`) sets four header fields after `Transfer`:

```
FILE_DESCRIPTION(('FreeCAD Model'),'2;1');
FILE_NAME('Open CASCADE Shape Model','2026-10-26T12:35:08',('FreeCAD'),('FreeCAD'),
  'Open CASCADE STEP processor 7.9','FreeCAD','Unknown');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));
```

`WriterStep::write` (`WriterStep.cpp:51-69`) sets author from the preference `Mod/Part/STEP/Author` (default "Author"), organization from `Company` (default empty), originating system from `App::Application::getExecutableName()` ("FreeCAD"), and the same description. The preferences page saves empty strings once it's opened; the oracle's FreeCAD has (`~/Library/Preferences/FreeCAD/v1-1/user.cfg`), so its `Import.export` writes `('')` for both. The dialog only accepts 7-bit ASCII for them (`7d0e06c961`, 2019).

Everything else is OCCT's (next section): the **name stays "Open CASCADE Shape Model"**, the **time stamp is the local time of the transfer**, preprocessor version OCCT's, authorisation "Unknown". FreeCAD used to set the name to the export path (`92ebfab47a`, 2014, which also brought the preferences), and stopped in `78c6cc1362` (2020, #4477) because a path with Cyrillic letters, written as raw UTF-8, made the file unreadable; the comment "Don't set name because STEP doesn't support UTF-8" (`TopoShape.cpp:909-910`, `WriterStep.cpp:59-60`) is that fix.

### Products, names, colours

- `exportStep` and `Part.export` write one product named by `write.step.product.name` plus OCCT's counter (below): `PRODUCT('Open CASCADE STEP translator 7.9 1', …)`. `Part.export` of several objects writes one product with several `MANIFOLD_SOLID_BREP`s.
- The XCAF path names each product by the object's `Label`, exactly (`TDataStd_Name`, `ExportOCAF2.cpp:112`), with one product per object; several objects become components (`NEXT_ASSEMBLY_USAGE_OCCURRENCE`) of an assembly product named by the document. Colours go in as XCAF colour attributes (per face, `ExportOCAF2.cpp:259-343`; `beea1d3644`, `c01878c174`, 2021). Body naming work on main (`f218c5f25c`, #25042, and its follow-ups, Nov 2025) was reverted on main (`7da646754a`, Dec 2025); 1.1.3 has neither.

### From FreeCAD's history

| Commit | What it fixed | For Silico |
|---|---|---|
| `7510657f2a` (2012) | a crash: the progress indicator attached to the reader, not the writer | nothing to port (no progress indicator) |
| `64890a4552` (2013, #1182) | names and colours lost on console-mode export | names come from the document, never from a GUI |
| `92ebfab47a` (2014) | header author, company, system from preferences | Silico writes them from the document instead (below) |
| `2f25170763` (2015) | the product name from a preference | product names are the features' names |
| `7901cf7ad2` (2017, #2698) | surplus PRODUCTs from an extra assembly node for one object | one body is one product, never wrapped |
| `1b26173580` (2019) | unit "IN" isn't an OCCT unit name; "INCH" is | Silico writes MM only |
| `7d0e06c961` (2019) | 7-bit ASCII only in header fields | Silico encodes every non-ASCII character instead (below) |
| `78c6cc1362` (2020, #4477) | raw UTF-8 in FILE_NAME made files unreadable | encode, never raw (below) |
| `c8c4686543` (2022, #6282) | `exportStep` after an XCAF export wrote assembly structure, because `write.step.assembly` is a process-wide static left at On | **pass parameters explicitly, never through statics** (below) |
| `70c18c8017` (2022) | pcurves off by default | not followed: on, for curved booleans' read-back (*Settings*, pcurves) |
| `9074fccc99` (2026, **main only**) | OCCT 8's STEP regression (OCCT #1327: curved faces lost): sets the shape-fix parameters and processing flags explicitly | not needed on 7.9.3: `STEPControl_Writer::Transfer` already sets the same defaults when none are set (`InitializeMissingParameters`, `STEPControl_Writer.cxx:282-295`); Silico's round trips below keep every face |

### FreeCAD's tests

There are almost none for export. `src/Mod/Import/TestImportGui.py:41-80` (`testSaveLoadStepFile`) exports a box with six face colours through `ImportGui.export` and reads them back; it needs the GUI, and Silico has no colours yet. Nothing tests the header, names, schemas, units, or round-trip geometry. The behaviours to port are therefore the ones the experiments below check, with FreeCAD 1.1.3 as the reader.

## What makes OCCT 7.9.3's bytes vary

### Per run: the time stamp, and nothing else

Every `Transfer` applies a default header (`STEPControl_Writer.cxx:145-146`; `STEPCAFControl_Writer` goes through its own `STEPControl_Writer`) built by `APIHeaderSection_MakeHeader::Init` (`APIHeaderSection_MakeHeader.cxx:86-142`):

| Field | OCCT's default | Varies |
|---|---|---|
| FILE_DESCRIPTION description | `('Open CASCADE Model')`, implementation level `'2;1'` (`:129-133`) | no |
| FILE_NAME name | `'Open CASCADE Shape Model'` (`:68-82`, `:94`) | no |
| time_stamp | **the current local time**, `Interface_MSG::TDate(…, 0,0,0,0,0,1, "C:%4.4d-…T…")` (`:98`), from `OSD_Process::SystemDate` (`gettimeofday` and `localtime`, `OSD_Process.cxx`) | **every second, and with `TZ`** |
| author | `('Author')` (`:102-105`) | no |
| organization | `('Open CASCADE')` (`:106-109`) | no |
| preprocessor_version | `'Open CASCADE STEP processor 7.9'` (`XSTEP_PROCESSOR_VERSION`, `Interface_Version.hxx:15`: `OCC_VERSION_STRING` is "7.9", not 7.9.3) | with OCCT's minor version |
| originating_system | `'Open CASCADE 7.9'` (`XSTEP_SYSTEM_VERSION`, `:16`) | with OCCT's minor version |
| authorisation | `'Unknown'` (`:121-122`) | no |
| FILE_SCHEMA | from the schema parameter (`StepAP214_Protocol.cxx:1584-1605`) | no |

Measured: the same box, written in two processes a second apart, differs in that one line; `TZ=Asia/Tokyo` and `TZ=UTC` in the same second give `2026-10-27T04:37:16` and `2026-10-26T19:37:16`. On conda-forge's build, each of the 36 default-header and FreeCAD-header configurations (9 shapes, plain and XCAF writers) gave 3 different files in 3 processes, and the first and third differ only in `FILE_NAME`'s line; on the recipe build, 4 different files in 4 processes. FreeCAD's own `exportStep` of the box, read back from FreeCAD 1.1.3, is byte for byte the probe's FreeCAD-mode file once the time stamp is masked.

### Per process: the plain writer's product counter

`STEPControl_Writer` names a part's product `WriteProductName` (or "Product" if empty) followed by its place in the assembly levels, " 1" at the first (`STEPConstruct_ContextTool.cxx:380-398`). The levels live in the write actor (`STEPControl_ActorWrite.hxx:126`, `myContext`), and the actor belongs to the STEP controller, one per process. Each transfer sets the depth to 1 without resetting the index, names the product, then counts the index up (`STEPControl_ActorWrite.cxx:572`, `:585`, `:605`); assembly transfers push and pop levels (`:1655`, `:1685`). So **a plain export's product name depends on every export the process made before it**:

```
plain twice identical: NO
  plain 1: = PRODUCT('Box 1'
  plain 2: = PRODUCT('Box 2'
  plain 3 (after two caf): = PRODUCT('Box 3'
caf twice identical: yes
```

(`twice.cpp`.) FreeCAD shows the same thing. In one session (`fcexport.py`), exporting four shapes each by `exportStep`, `Part.export`, and `Import.export`, the plain exports' products were numbered 1, 2, 2, 3, 5, 6, 8, 9: the XCAF exports between them, with assembly mode On, moved the index too. A long-lived kernel worker would give the same request different bytes depending on what it served before. `STEPCAFControl_Writer` names products from their labels and is immune: an XCAF export first in a fresh process and one after three plain and two XCAF exports are byte-identical.

### Per process: the statics

`Interface_Static` values are process-wide. A worker that sets `write.step.schema` for one request leaves it set for the next: FreeCAD's #6282 is this bug. OCCT 7.9 copies them into the model at `Transfer` (`DESTEP_Parameters::InitFromStatic`, `DESTEP_Parameters.cxx:24-80`, its write fields at `:59-79`), but its `Transfer(…, const DESTEP_Parameters&, …)` overloads (`STEPControl_Writer.hxx:99-101`, `STEPCAFControl_Writer.hxx:105-107`) take them directly and read no write static. What's still read from statics on the write path: `xstep.cascade.unit` (MM, `XSAlgo.cxx:60`, used through `UnitsMethods::GetCasCadeLengthUnit`) and the shape processor's resource names; a worker that never sets them gets OCCT's defaults.

### Entity order: deterministic

STEP's `#n` numbers are the model's entity order. The model is filled by `Interface_InterfaceModel::AddWithRefs` (`Interface_InterfaceModel.cxx:551-581`), a depth-first walk of each transferred root's references in field order, into an indexed map (insertion order), from `TransferFinder` (`XSControl_Controller.cxx:243-276`), plus the entities the actor adds in sequence. The shapes are walked with `TopExp_Explorer` and `TopoDS_Iterator`, in the shape's own order. Two address-keyed iterations are on the path, and neither orders entities:

- `XSAlgo_ShapeProcessor::MergeShapeTransferInfo` (`XSAlgo_ShapeProcessor.cxx:217-300`) iterates a `TopTools_DataMapOfShapeShape`, keyed by `TShape` address, at `:228`. It runs after the writer's shape processing (`STEPControl_ActorWrite.cxx:1339`) and binds results into the `Transfer_FinderProcess`, whose later uses are lookups. It adds no model entity.
- `STEPCAFControl_Writer` iterates `myRootLabels`, a `TDF_LabelMap` keyed by label-node address (`STEPCAFControl_Writer.cxx:1672`, `:3896`, …). For one document it holds one root (`:399-400`).

Measured: every shape, both writers, with every header field and parameter fixed: one hash across four processes each, with ASLR (the probes are PIE: `main` at 0x104f63f48, 0x104377f48, 0x102c37f48 in three runs) and with each process first allocating 0 to 111 heap blocks of assorted sizes, so that every OCCT object lands elsewhere.

### Per platform: floats, locale, line endings

- **Format.** `StepData_StepWriter` formats reals with `Interface_FloatWriter(12)` (`StepData_StepWriter.cxx:212`): `%14.12f` for |v| in [0.1, 1000), `%14.12E` otherwise, then strips trailing zeros and `E+00` (`Interface_FloatWriter.cxx:47-63`, `:102-…`). So values in [0.1, 1000) keep 12 decimals, and other values keep 13 significant digits: 123456.789012345678 is written `1.234567890123E+05`. **A STEP file isn't an exact copy of the B-rep**, in FreeCAD as in Silico (see *Edge cases*). `-0.` is written as such, so the sign of a zero reaches the bytes.
- **Locale.** `Sprintf` formats in the C locale whatever the process's locale is: `vsprintf_l` on macOS, a `Standard_CLocaleSentry` (`uselocale`) on glibc (`Standard_CString.cxx:31-58`, `:91-98`).
- **libc.** The formatting is libc's `printf`, outside the libm shim. Both macOS's and glibc's are correctly rounded. Measured: 2,000,000 doubles (random bit patterns, uniform values in ±2000, ±10^k for k from -12 to 12, and values a hair off a tie at the 12th decimal, from a fixed splitmix64 generator) formatted with OCCT's two formats give byte-identical output on macOS arm64 (libSystem) and Linux x86-64 (glibc 2.41), SHA-256 `2dddb11d…`. (A first attempt through Python's `ctypes` failed on macOS arm64, where variadic arguments are passed on the stack; the C program is the measurement.)
- **Line endings.** The writer emits `\n`. `WriteStream` (`STEPControl_Writer.cxx:171-191`, `STEPCAFControl_Writer::WriteStream`) gives the same bytes as `Write(file)` (measured, both writers). Windows isn't a checked platform.
- **Version strings.** OCCT's preprocessor version and the default product name carry `OCC_VERSION_STRING`; an OCCT upgrade changes them, as it already changes the kernel's identity.

### Across platforms, measured

The same exact B-reps (OCCT's text format writes 17 significant digits, `GeomTools_*Set.cxx`, so a read is exact, signed zeros too), written with the fixed recipe by `linux/readprobe.cpp`:

| Build | Platform | 17 files (box, fillet, B-spline pad, 3-solid compound, tiny, huge, offset, INTERNAL face, UTF-8 name; plain and XCAF writers) |
|---|---|---|
| Silico's recipe (this OCCT, patches, `-ffp-contract=off`, libm shim), with `BUILD_ADDITIONAL_TOOLKITS=TKDESTEP` | macOS arm64 | reference |
| conda-forge `occt 7.9.3 novtk_h1713e6e_103` (the oracle's) | macOS arm64 | **identical, 17 of 17** |
| conda-forge `occt 7.9.3 novtk_hb176d5c_103`, GCC 15.3, glibc 2.41 | Linux x86-64 (Docker, `linux/amd64`) | **identical, 17 of 17** |

This checks the writer, not the recipe's geometry: the input is the same B-rep on every side, and the Linux side is stock OCCT. That the shapes themselves match across platforms is the recipe's own claim (`pixi run stress`). A recipe build on Linux wasn't made: STEP's toolkits double the OCCT build (below), and full runs belong on CI. The first CI run of the export's tests on both platforms is the check that remains.

## The recipe for deterministic bytes

1. **Write with `STEPCAFControl_Writer`**, from an XCAF document built per request (`new TDocStd_Document("XmlXCAF")`, no application, no storage drivers needed), never `STEPControl_Writer`, whose product counter runs per process.
2. **Pass `DESTEP_Parameters` to `Transfer(doc, params, STEPControl_AsIs)`**, every field the export cares about set: `WriteSchema = AP214IS`, `WriteUnit = Millimeter`, `WriteSurfaceCurMode = true` (as built: see *pcurves* below), `WriteAssembly = Auto`, `WritePrecisionMode = Average`, `WriteColor`/`WriteName` true, the rest OCCT's defaults. Never set an `Interface_Static`.
3. **After `Transfer`, overwrite every header field** through `APIHeaderSection_MakeHeader(writer.ChangeWriter().Model())`: `SetName`, `SetTimeStamp`, `SetAuthorValue(1, …)`, `SetOrganizationValue(1, …)`, `SetPreprocessorVersion`, `SetOriginatingSystem`, `SetAuthorisation`, `SetDescriptionValue(1, …)`. The schema and the implementation level ('2;1') come from the parameters and are fixed.
4. **Write with `WriteStream`** into memory, and return the bytes; the core writes the file (a new file renamed into place, as the 3MF does).
5. **Encode every string before OCCT sees it** (the next section), so OCCT's escaping is never asked to guess.

With 1 to 4, all 18 configurations of `probe.cpp` (`fixed` mode) and the 17 reader-probe files gave one hash per configuration, across processes, heap layouts, OCCT builds, and platforms. The header written:

```
FILE_DESCRIPTION((''),'2;1');
FILE_NAME('bracket.silico','1970-01-01T00:00:00',(''),(''),
  'Open CASCADE STEP processor 7.9','Silico','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));
```

(The probes wrote 'Silico model' and 'Silico (OCCT 7.9.3)' in two of these fields; the recommendation below differs only in those strings.)

## Strings: how names reach the file exactly

OCCT 7.9 escapes strings itself (`cleanTextForSend`, `StepData_StepWriter.cxx:88-205`, used for every string, header fields included, `:978-985`, `RWHeaderSection_RWFileName.cxx:120-154`): `'` becomes `''`, `\` becomes `\\`, newline `\N\`, tab `\T\`, and everything else is copied byte for byte. **It also passes through anything that looks like an ISO 10303-21 directive** (`\X2\…\X0\`, `\X4\…\X0\`, `\XHH\`, `\S\`, `\N\`, `\T\`, `\Px\`), so a name is ambiguous. Read back by FreeCAD 1.1.3 (`Import.insert`, the product name as the object's `Label`):

| Name given to OCCT | Written | FreeCAD reads |
|---|---|---|
| `Klötzchen 'q' \ — end` (UTF-8) | raw UTF-8 bytes, `''`, `\\` | exact (FreeCAD's OCCT reads raw UTF-8; edition-2 readers needn't) |
| `two⏎lines⇥and tab` | `two\N\lines\T\and tab` | exact |
| `a\N\b` (a literal backslash, N, backslash) | `a\N\b`, untouched | **`a⏎b`: wrong** |
| `C:\X2\temp` | `C:\X2\temp`, untouched | **`C:\u0e00`: wrong** |
| `a\X2\005C\X0\N\X2\005C\X0\b` (backslashes encoded) | as given | `a\N\b`: exact |
| `Kl\X2\00F6\X0\tzchen` | as given | `Klötzchen`: exact |
| `smile \X4\0001F600\X0\ end` | as given | `smile 😀 end`: exact |
| `it's` | `it''s` | exact (but `'` before `,` or `)` isn't: encoded, as built) |

So Silico encodes every string itself, in Rust where it's testable without OCCT, and never hands OCCT one (as built, from `/break-it`): OCCT breaks a string past 72 columns where it can (after a space, else a backslash, else an `_`, else at column 72), inside an encoded run too, and its reader decodes the run before it joins the lines, so a long name came back as `?`; it does so in time quadratic in the string's length (a 4 MiB name took 36 s); and its reader ends a string at a doubled `'` before `,` or `)`, so `Bracket 'A', left` came back as `Bracket 'A'`. The worker gives OCCT a short token for each string (`SILICO-NAME-0`, `SILICO-ASSEMBLY-0`, `SILICO-FILE-0`), and splices the encoded string in where each token is, once the file is written, in one pass, every token found or refused. The splice is the worker's Rust (`silico-step-worker.rs`, `Strings`), not its C++, which gives OCCT the tokens it's handed and returns OCCT's text unchanged. The encoding: every character from U+0020 to U+007E except `\` and `'` as itself; every other character (backslash, control characters, all non-ASCII), in maximal runs, as `\X2\` and four hex digits each, then `\X0\`, using `\X4\` with eight hex digits for a character above U+FFFF (ISO 10303-21:2002 §6.4.3.3: `\X2\` is UCS-2, so no surrogate pairs). Uppercase hex, so the bytes are unique. The worker checks that what it's given is in that form (a protocol `check`, as `Primitive::check` does) and refuses anything else. With this encoding **every name a document can hold can be written**; nothing is refused for its characters, unlike 3MF's XML (docs/ports/mesh-export.md refuses control characters).

## "Header written from the document": recommendations

| Field | Recommendation | Why | FreeCAD 1.1.3 |
|---|---|---|---|
| FILE_NAME name | the **document's file name**, its last component only (`bracket.silico`), encoded | It ties the STEP to its source, which is what a git-native user wants to trace. It doesn't depend on where the export is written, so the same model gives the same bytes at any path and the worker never sees the path. A directory would leak the user's home path. ISO 10303-21 means the exchange file's own name, which a rename invalidates anyway. SolidWorks writes the exported file's name, as far as I know (not checked here). | `'Open CASCADE Shape Model'` (OCCT's); before 2020, the export path, raw |
| time_stamp | **`'1970-01-01T00:00:00'`**, fixed | Any clock makes the bytes vary. The document holds no time, and the git commit time is outside it (and absent before a commit). ISO 10303-21 asks for an ISO 8601 time, so `''` would fail a strict reader. The Unix epoch is the usual "no time" (`SOURCE_DATE_EPOCH=0`). OCCT writes no zone, so neither does Silico. | local time of the transfer |
| author | `('')` | Silico has no user identity in the document, and STEP allows an empty string (the list needs one element). | preference, default `('Author')`; `('')` once the page is saved |
| organization | `('')` | as author | preference `Company`, default `('')` |
| preprocessor_version | **OCCT's own, `'Open CASCADE STEP processor 7.9'`**, set explicitly from `XSTEP_PROCESSOR_VERSION` | It is the translator that wrote the file. It changes only with OCCT, which changes the geometry's bytes and the kernel's identity anyway. Not the worker's identity hash, which changes every build. | the same |
| originating_system | **`'Silico'`** | the system the data came from; without a version, so a release that changes no geometry changes no byte | `'FreeCAD'` |
| authorisation | `''` | nothing is authorised; Silico doesn't invent values | `'Unknown'` (OCCT's) |
| FILE_DESCRIPTION | `('')`, level `'2;1'` | it would only repeat originating_system | `('FreeCAD Model')` |
| Product names | each body's **feature `name`**, encoded; the PRODUCT's `id` the **feature ID** (`f0000003`) | The name is what a user sees, and FreeCAD reads it as the `Label`. The ID is unique and survives renames, as the 3MF's `partnumber` is (docs/ports/mesh-export.md). OCCT writes id = name; the worker sets each `StepBasic_Product`'s id after `Transfer`, in model order, which is the XCAF document's label order (measured: `PRODUCT('f0000002','Box',…)`, `PRODUCT('f0000003','Box',…)`; FreeCAD then reads `Box`, `Box001`). | the `Label` as both id and name |
| Several bodies | **an assembly product named by the document's file stem**, id `''`, each body a component | FreeCAD's GUI path does this, and a reader keeps the bodies apart | the same, the assembly named by the document's internal `Name` |
| Schema | **AP214IS** (`AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }`) | It is FreeCAD's default and OCCT's, so Silico's file is FreeCAD's in everything but the header. Every reader reads it. OCCT 7.9.3's AP242 option is `AP242DIS`, and for B-rep solids it writes the same entities under another schema name, so it would gain nothing until PMI exists. FreeCAD reads all five of OCCT's schemas back the same (2e-14). | AP214IS |
| Units | **mm** (`SI_UNIT(.MILLI.,.METRE.)`), no option | The document is in mm. A unit option would only rescale the numbers, and lose digits doing it. | preference, default mm |
| pcurves | **on** (`WriteSurfaceCurMode = true`), as built | Recommended off at first: FreeCAD's default since `70c18c8017` (which gives no reason), files 40 to 60% smaller, and the study's read-backs (a box, fillets, a B-spline pad) closer without them (4e-16 against 3e-14). Built, the oracle's whole case set read back through FreeCAD showed the other side: without pcurves the reader rebuilds them, and two spheres fused read back 1.1e-6 off in volume (a sphere common a box 4e-10); with them, 4.5e-11 and exact to 1e-10. Exactness over size: on. | off |
| Uncertainty | OCCT's `Average` (each part's mean tolerance) | FreeCAD's; deterministic, the shape's own tolerances | the same |

## Round trips through FreeCAD 1.1.3

Each file read back with `Part.read` (`TopoShape::importStep`) and `Import.insert` (ImportOCAF2, for names) in `freecadcmd`, against the original B-rep read by FreeCAD (`roundtrip.py`, `rt/roundtrip.txt`, `rt2/`, `rt3/`, `rt4/`):

| Shape | Faces | Volume, relative error (recipe / FreeCAD settings) | Area, relative error | Valid | Names |
|---|---|---|---|---|---|
| box 10×20×30 | 6 → 6 | 0 / 0 | 0 / 0 | yes | `Box` |
| filleted box, r 2 on all 12 edges | 26 → 26 | 1.9e-14 / 0 | 1.1e-14 / 0 | yes | `Fillet` |
| B-spline face swept 15 mm | 6 → 6 | 0 / 1.3e-16 | 1.1e-14 / 1.1e-14 | yes | `Surface pad` |
| three bodies (box, cylinder, filleted box) | 35 → 35, 3 solids | 3.1e-14 / 1.4e-16 | 1.3e-14 / 0 | yes | `Box`, `Cylinder`, `Fillet` |
| one feature of three solids | 35, 3 solids | 3.1e-14 | — | yes | `Two lumps`, one product |
| filleted box ×1e-6 | 26 → 26 | 1.9e-14 / 2.7e-16 | 1.1e-14 / 2.1e-16 | yes | |
| filleted box ×1e6 | 26 → 26 | 1.9e-14 / 3.6e-16 | 1.2e-14 / 0 | yes | |
| filleted box at (123456.789…, 1/3, -0.1) | 26 → 26 | 3.0e-14 / 0 | 2.0e-14 / 0 | yes | |
| filleted box at (1.23e7, -9.88e6, 0.3) | 26 → 26 | 1.6e-14 / 1.3e-14 | 2.0e-14 / 1.1e-16 | yes, tolerances still 1e-7 | |
| box with an INTERNAL face (General Fuse) | **7 → 6** | 0 | **8.3e-2** (the face's area) | yes | |

The plain writer's files read back the same, named `Box 1`, `Fillet 1`, … (the counter).

As built, the oracle's whole case set read back through FreeCAD 1.1.3 (`silico-oracle`'s `step_oracle`): every case within 1e-10 in volume and area, where it was (its tight box and center within 1e-9 of its diagonal), its faces and face bounds all written, its name its label; and two parts placed apart, each at its box by hand.

**A name with no ASCII graphic character reads back as the product's id on Linux** (an OCCT reader bug, followed and said, not worked around; upstream draft for the maintainer to post). `STEPCAFControl_Reader::ReadNames` takes a product's name only if `UsefullLength() > 0`, which counts bytes `isgraph` accepts; glibc's C and UTF-8 locales accept no byte past ASCII, and macOS accepts most. So 支架, `ü`, or 😀 alone is read on Linux as `f0000003`, its id; a name holding any ASCII graphic character (`Klötzchen`) reads back exactly everywhere. The file holds the name exactly either way (`adversarial_step_oracle`).

## How it fits Silico

### The build

`TKDESTEP` (STEP, and `STEPCAFControl`) links `TKXCAF`, which links `TKV3d` and `TKService` (Visualization), `TKCAF`, `TKLCAF`, `TKCDF`, `TKVCAF`, `TKXSBase`, `TKDE`. `third_party/occt/build.sh` builds with `BUILD_MODULE_DataExchange=OFF`. The smallest change is `-DBUILD_ADDITIONAL_TOOLKITS=TKDESTEP`, which pulls in exactly its dependencies (OCCT's CMake resolves them, `CMakeLists.txt:406-428`). Measured with the recipe:

- 4,701 build steps against 2,220 now; about 20 minutes at `-j 4`, `nice`.
- About 15 MB more libraries (`TKDESTEP` 7 MB). It changes no existing library, so the stress reference stays byte-identical.
- No new inexact math imports: `nm -u` on all ten new libraries finds none of the shim's functions.
- On macOS, `TKService` links AppKit, IOKit, CoreGraphics, Foundation, and CoreFoundation, so whatever links it loads them. As built, that's the STEP worker alone (`silico-step-worker`, in silico-kernel, which stays the only crate that links OCCT): linked into the kernel worker, they took its startup from about 30 to 70 ms and its memory from 8.4 to 18.5 MB (debug), past what the pool's rule for stopping superseded work assumes, in every worker of the pool. The STEP worker is started for each export (exports are rare, and a fresh process is deterministic by construction), its own bridge (`cpp/step.cpp`) and its own import check; the kernel worker links none of STEP's toolkits, tested.
- On Linux, `TKService` includes X11's headers unless OCCT is configured with `USE_XLIB=OFF`, as build.sh does (nothing Silico calls draws).
- The writer prints transfer statistics to stdout through `Message::DefaultMessenger()`. The worker removes the default printers (or routes them to `tracing`).

### The worker's request

`Op::ExportStep`, answered by the STEP worker (the kernel worker refuses it as another worker's):

```
ExportStep {
  parts: [ { shape: <a body's B-rep>, id: "f0000003", name: "<encoded>" } ],  // the file's order, at least one
  assembly: "<encoded document stem>",   // used only with two or more parts
  file_name: "<encoded document file name>"  // the header's; its other fields the worker's constants
}
→ StepFile {
  step: <bytes>,
  left_out: [ [<face numbers>], ... ],          // a list a part: its INTERNAL and EXTERNAL faces
  unwritten: [ { part, element, messages } ],   // the writer's check-list failures, of anything else
  faces: { expected, written },                 // the parts' faces that bound material, and the file's
  bounds: { expected, written },                // their wires, and the file's face bounds
}
```

The core encodes the strings, names the parts, and words refusals (`command::export`'s `unwritten`); the worker checks the strings' form (`ExportStep::check`) and each part's B-rep (as the kernel does), builds the XCAF document, transfers with the parameters above, sets the ids and the header, splices in the strings, and reports what it finds. Whatever wrote a file, its identity (the STEP worker's build: `STEP worker sha256:...`) is logged with it, as a kernel's keys its results.

Refused:

- **No parts** (`features: []`, or a document with no body): `invalid_input`, naming `features`, as the 3MF export refuses it (after FreeCAD's mesh export, `04005b5e45`). FreeCAD's STEP export doesn't refuse: `ExportOCAF2::exportObjects` returns at once on an empty list (`ExportOCAF2.cpp:347-349`) and the writer is handed an empty document (what it writes then is not verified).
- **A feature that didn't build**: its own failure, as `describe` answers it.
- **A string not in the encoded form**: a protocol error (the core never sends one).
- **A transfer OCCT fails** (`Transfer` returns false, or `WriteStream` fails): `invalid_result`, naming the feature if there's one, with OCCT's message.
- **A check-list failure, or fewer faces or face bounds written than the parts have**: `invalid_result`, naming the feature (one part's) and the elements, the core's words.
- **No STEP worker to be had**: `worker_unavailable`, saying how to build it or name it (`SILICO_STEP_WORKER`).
- **Names: nothing.** Every Unicode string can be encoded. A name's length is bounded by the document's (16 MiB).

`export`'s `tolerance` means nothing for STEP: decided (Fable, 2026-10-26), it's `null` for STEP, and a length is refused naming it, as a camera's `view_height` is for a perspective camera.

### Edge cases

- **INTERNAL and EXTERNAL faces.** OCCT's writer drops them with only a transfer warning ("Face from Non Manifold Topology", `TopoDSToStep_MakeStepFace.cxx:143-149`), and FreeCAD's export does the same with no word. Silico finds them before the transfer and reports them in `left_out`, as the 3MF export does (decided for 3MF in m4.8-export-fable). The solid's volume is unchanged (OCCT's volume ignores them too) and its area loses theirs.
- **Several solids in one feature**: one product, one `MANIFOLD_SOLID_BREP` each (measured, `rt4/compound-one-feature.step`).
- **Assembly links**: OCCT numbers each `NEXT_ASSEMBLY_USAGE_OCCURRENCE` by a counter that runs for the life of the process (`STEPConstruct_Assembly::MakeRelationship`), so a worker's second assembly read `'3'`, `'4'`: the worker sets each link's id to its place among them, from 1 (found by `/break-it`).
- **Two solids sharing a face** (a general fuse's): the writer writes the face once for each solid, so the worker counts it once for each solid too, and the counts must match exactly. A face shared by a solid and a free shell, or by two free shells, is written more than once and counted once, so it's refused: no feature makes one, and a refusal is safer than geometry missing unsaid.
- **Two features with the same name**: two products with the same name and different ids (allowed); FreeCAD renames the second `Box001` on import.
- **Shared shapes.** `XCAFDoc_ShapeTool::AddComponent` reuses a label for a shape it already holds (the same `TShape`), which would make two bodies instances of one product. Bodies a worker builds or reads separately never share a `TShape`; the request should still be built so that each part is its own (`AddShape(…, makeAssembly = false)` per part, then `AddComponent` of that label).
- **Very small and very large coordinates.** The 12-decimal and 13-significant-digit format loses up to 5e-13 mm absolute in [0.1, 1000) mm and 5e-14 relative outside it. At 1.2e7 mm that moves a vertex by 4.5e-6 mm, past OCCT's 1e-7 confusion, yet the read-back is valid and within 2e-14 in volume. Parts at the kernel's length limits (1e8 mm) lose up to 5e-6 mm. That is FreeCAD's precision too. A 17-digit format isn't available safely: `StepData_StepWriter::Send(Real)` formats into `char lval[24]` (`StepData_StepWriter.cxx:968`), which a 17-digit `%E` of a negative value with a three-digit exponent overflows, and the zero-suppression scans only 16 characters (`Interface_FloatWriter.cxx:127`). Silico follows FreeCAD and says so in the export's documentation. It can't be fixed under the patch policy, which allows only determinism and crash fixes.
- **Non-finite values** can't reach the writer: the kernel refuses non-finite geometry.
- **Faces in no solid** (a shell or free faces in a body, as some of the NIST corpus has) would be written as a shell-based surface model under `AsIs`. Silico's bodies are solids, so this waits for import.
- **Timeouts.** The transfer runs inside the kernel request's 60 s, as the 3MF mesh does.
- **What the writer reports** (as built, refining Fable's residual-risk item): every failure on the transfer's check list, of anything but a face left out, is reported for the core to refuse, naming the part and the element. Its warnings are not: it warns of what it corrects as it writes ("Direction of Face of revolution corrected" on every cylinder), so they're logged. What makes a dropped face loud whatever the messages say is a count: the file must hold as many face surfaces as the parts have faces that bound material, or the export is refused.

### Tests to write (expected values from FreeCAD 1.1.3 or by hand)

- **Determinism**: each case written twice, in two worker processes and again after other exports in one process; byte-identical. A golden SHA-256 per case, the same on macOS arm64 and Linux x86-64 in CI (the cross-platform check this note couldn't run on the recipe's Linux build).
- **The header**: every field exactly as recommended, by reading the bytes; and the time stamp independent of `TZ` and of the clock.
- **Round trip through the oracle**: box, filleted box, B-spline sweep, a sketch pad and pocket from the corpus; volume, area, face count, and validity read back by FreeCAD 1.1.3 equal the original's measures as the oracle compares them (`measure_as_freecad`), to the export's rounding (2e-14 relative measured here).
- **Names**: the table under *Strings* as cases, read back by FreeCAD as `Label`s; ids are the feature IDs.
- **Several bodies**: an assembly with one component per body, in timeline order.
- **Edge cases**: an INTERNAL face in `left_out` and absent from the file; ×1e-6 and ×1e6 parts and a part at 1e7 mm valid on read-back; no parts refused.
- **The encoder** (Rust, no OCCT): every character class, maximal runs, `\X4\` for astral characters, uppercase hex; property test: decoding the encoded form, as ISO 10303-21 reads it, gives the name back.

## Where Silico differs from FreeCAD on purpose

- The header is fixed by the document, not by the clock and preferences.
- Product ids are feature IDs; FreeCAD writes the label twice.
- Names keep every character, encoded; FreeCAD writes raw UTF-8 and leaves the file name out because of it.
- INTERNAL faces are reported; FreeCAD drops them silently (both drop them).
- No unit, schema, or pcurve option. FreeCAD's defaults are taken as the only values, but pcurves, written (FreeCAD's default omits them).
- A part placed by a location is one product. XCAF makes a located shape a reference to a prototype (two products for one body, the inner one named `SOLID`), so the worker gives it in a compound of its own.

## Experiments

| File | What |
|---|---|
| `build-occt.sh`, `build.log`, `occt-install/` | OCCT 7.9.3 with Silico's patches and recipe, plus `BUILD_ADDITIONAL_TOOLKITS=TKDESTEP` |
| `probe.cpp`, `compile.sh`, `run.sh` | 9 shapes × {default, FreeCAD, fixed} header × {plain, XCAF} writer, N processes each, heap perturbed (`PERTURB`) |
| `stock-summary.txt`, `recipe-summary.txt`, `out/` | the matrix on conda-forge's OCCT (3 runs) and on the recipe build (4 runs): default and FreeCAD modes vary every run, fixed never |
| `twice.cpp` | the plain writer's per-process product counter; the XCAF writer's independence from earlier exports |
| `fcexport.py`, `fc/` | FreeCAD 1.1.3's own `exportStep`, `Part.export`, `Import.export` of the same shapes |
| `linux/readprobe.cpp`, `linux/run.sh`, `linux/in-container.sh`, `out-mac-recipe/`, `out-mac-stock/`, `linux/out-linux-x86_64/` | the same B-reps written on macOS (recipe and stock) and Linux x86-64: 17 of 17 identical |
| `printf/fmt.c`, `printf/mac-out.txt`, `printf/linux-out.txt` | OCCT's float formats over 2,000,000 doubles, macOS libc vs glibc: identical |
| `roundtrip.py`, `labels.py`, `far.py`, `comp.py`, `rt*/` | round trips and name read-back through FreeCAD 1.1.3 |
| `readprobe2.cpp` | schemas, name encodings, product ids set after transfer |

## Not verified

- A recipe build of the STEP toolkits on Linux; the cross-platform check used conda-forge's OCCT on Linux, with the shape read from the same exact B-rep.
- Readers other than FreeCAD 1.1.3 (OCCT 7.9.3): SolidWorks, Fusion 360, and older OCCT on raw UTF-8 vs `\X2\` names; the recommendation (`\X2\` always) is the spec's form, so it shouldn't matter.
- Whether SolidWorks writes the exported file's name in FILE_NAME.
- Linux's `TKService` dependencies with Visualization's options off.
- Colours: Silico has none yet; the XCAF writer writes no styles without them (measured: no `COLOUR_RGB` in any file).
