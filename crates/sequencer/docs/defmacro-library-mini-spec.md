# Defmacro Library Mini Spec

## Purpose

Make patcher `defmacro` subpatches reusable across instruments and effects
without copying their bodies into every patch.

The target workflow is close to Max abstractions:

- create a local `defmacro` in a patch;
- enter the macro view and edit it visually;
- save that macro to a shared library intentionally;
- use the saved macro from other patches through autocomplete;
- edit the shared macro from a library macro view when shared propagation is
  intended;
- fork a library macro back into the current patch when local divergence is
  intended.

## Non-Goals

- Automatically importing every `defmacro` found in saved patches.
- Silently copying library macro bodies into the current patch as the default
  behavior.
- Storing semantic macro dependencies only in layout sidecars.
- Changing the DGenLisp parser/compiler before there is a clear need. V1 can
  materialize imports before sending source to the existing compiler.
- Supporting multiple public macros per library package in V1.

## Source Form

Imported library macros are represented in patch source with an explicit
semantic form:

```lisp
(use-defmacro karplusstr2)
(use-defmacro pitch2freq)
```

`use-defmacro` is host/patcher metadata. It is not emitted to DGenLisp.

Before compiling, probing, or showing generated source, the host materializes
the patch by replacing the import set with concrete `defmacro` definitions:

```lisp
(defmacro pitch2freq (pitch)
  ...)

(defmacro karplusstr2 (exc freq decay damp)
  ...)

(param freq 440)
(def voice (karplusstr2 exc (pitch2freq freq) 0.9 0.4))
(out voice)
```

The user-authored `dsp.lisp` should keep the explicit `(use-defmacro ...)`
forms. The materialized source is a generated view for DGenLisp and debugging.

## Library Layout

V1 library macros live in a repo-owned folder:

```text
content/defmacros/
  karplusstr2/
    macro.lisp
    macro.layout.json
    manifest.json
  pitch2freq/
    macro.lisp
    macro.layout.json
    manifest.json
```

`macro.lisp` is the source of truth and must contain exactly one public
`defmacro` whose name matches the package directory:

```lisp
(defmacro karplusstr2 (exc freq decay damp)
  ...)
```

`macro.layout.json` stores the visual layout for the library macro's patcher
view. It is non-semantic.

`manifest.json` is an index/cache aid for autocomplete and discovery. It may
include:

```json
{
  "version": 1,
  "name": "karplusstr2",
  "params": ["exc", "freq", "decay", "damp"],
  "outputs": ["out"],
  "summary": "Karplus-Strong resonator",
  "tags": ["physical-model", "string"]
}
```

The manifest must be rebuildable from `macro.lisp`; it must not become the only
source of semantic truth.

## Library Macro Dependencies

Library macros may depend on other library macros:

```lisp
(use-defmacro pitch2freq)
(use-defmacro karplusstr2-core)

(defmacro karplusstr2 (exc pitch decay damp)
  (karplusstr2-core exc (pitch2freq pitch) decay damp))
```

The materializer resolves imports transitively:

1. Parse the patch's local `defmacro` definitions and direct
   `(use-defmacro ...)` forms.
2. Load requested library macro packages from the library index.
3. Recursively load each library macro's own `(use-defmacro ...)` forms.
4. Detect cycles and report a clear diagnostic with the import chain.
5. Topologically emit library definitions before patch forms that use them.
6. Emit each resolved macro definition at most once.

Cycles are save/compile blockers:

```text
defmacro import cycle: karplusstr2 -> resonator-core -> karplusstr2
```

Missing imports are save/compile blockers:

```text
unknown library defmacro `pitch2freq`
```

## Name Resolution

DGenLisp macro names are symbol-based, so V1 uses one resolved definition per
macro name in a materialized source.

Resolution rules:

- Local `defmacro` definitions in the current patch win over library macros
  with the same name.
- Local definitions also win over transitive library dependencies with the
  same name.
- Direct imports whose names are shadowed by local definitions are allowed but
  not materialized; the local definition is the resolved macro.
- Two library packages with the same public macro name are an index error.
- A library macro whose public name collides with a DGenLisp builtin is invalid
  unless the compiler already supports that operator/macro shadowing safely.

The patcher should surface shadowing in diagnostics or import metadata, but it
must be deterministic and honest. No hidden "best guess" copy should be made.

Future namespacing can provide stronger dependency isolation, but V1 should not
fake isolation if the compiler macro namespace is global.

## User Workflow

### Local To Library

Starting source:

```lisp
(defmacro mine (input)
  (* input 1))

(def y (mine x))
```

User enters the `mine` macro view. Because `mine` is local, the view shows a
`Save macro to library` action.

When clicked:

- create `content/defmacros/mine/macro.lisp`;
- create `content/defmacros/mine/macro.layout.json`;
- create or rebuild `manifest.json`;
- replace the local `defmacro` in the current patch with
  `(use-defmacro mine)`;
- preserve existing macro instance nodes as calls to `mine`;
- rebuild the library autocomplete index.

After this operation, the same view becomes a library macro view and the action
changes to `Fork`.

### Library To Local Fork

When viewing a library macro from any patch, the view shows `Fork`.

When clicked:

- copy the library macro source into the current patch as a local `defmacro`;
- copy the library macro layout into the current patch's `dsp.layout.json`
  macro namespace;
- remove `(use-defmacro name)` from the current patch if no remaining imported
  library macro requires it directly;
- keep existing macro instance nodes pointed at the same macro name;
- switch the active view to the new local macro.

After forking, the action changes back to `Save macro to library`.

If a local macro with that name already exists, fork is blocked unless the user
chooses an explicit rename flow.

## Patcher Views And Writeback Ownership

The active patcher view must carry an explicit source target:

```rust
enum PatcherViewSource {
    PatchRoot { dsp_path: PathBuf },
    LocalMacro { dsp_path: PathBuf, macro_name: String },
    LibraryMacro {
        macro_path: PathBuf,
        layout_path: PathBuf,
        macro_name: String,
    },
}
```

Writeback uses this target instead of inferring ownership from node names.

- `PatchRoot` writes the root patch source and root layout.
- `LocalMacro` rewrites the owning `defmacro` inside the current patch and the
  current patch layout sidecar.
- `LibraryMacro` rewrites the library `macro.lisp` and
  `macro.layout.json`.

The breadcrumb should make ownership visible. For example:

```text
root / dsp.lisp / mine
library / mine
```

or:

```text
root / dsp.lisp / mine [library]
```

Editing shared library code must never look identical to editing a private
local macro.

## Autocomplete

Autocomplete candidates come from:

- DGenLisp builtins;
- local `defmacro` definitions in the current patch;
- already imported library macros;
- library macros in the cached library index.

Candidate ordering:

1. exact/prefix local macro matches;
2. exact/prefix imported library matches;
3. exact/prefix unimported library matches;
4. builtin/operator matches;
5. substring/fuzzy matches within the same groups.

Autocomplete UI should label provenance:

```text
mine           local
karplusstr2    library
pitch2freq     library
```

Accepting an unimported library macro:

- inserts or renames the macro instance node;
- adds `(use-defmacro name)` to the patch source/import set;
- triggers materialized-source validation.

Accepting a local macro just inserts or renames the node.

## Library Index

The library index is built from `content/defmacros/*`.

It should be rebuilt:

- once at app startup or first patcher use;
- after saving a macro to the library;
- after editing a library macro;
- when a file watcher observes changes under the defmacro library directory.

The index should not be rebuilt on every autocomplete keystroke.

Index failures should be visible but scoped. A broken library macro should not
prevent unrelated local patch editing, but importing or using that broken macro
must block materialization/compile with a clear diagnostic.

## Materialization Call Sites

All host paths that compile or inspect DGenLisp patch source should use the same
materialization function:

- app compile/load path;
- `instrument_probe`;
- effect probe or future probe tools;
- emitted source viewer;
- patcher validation tests.

This avoids behavior where a macro works in the patcher but fails in the host
or probe path.

## Save And Compile Blockers

Block save/compile when:

- a `(use-defmacro ...)` target is missing;
- import resolution has a cycle;
- a library package contains zero or multiple public macros;
- a library package public macro name does not match its package name;
- a fork would overwrite an existing local macro without an explicit rename;
- writing a library macro would overwrite an existing package without an
  explicit overwrite/rename flow;
- a library macro's layout cannot be read or written when saving visual edits.

## Tests

Minimum test coverage:

- parse patch source containing `(use-defmacro name)` without projecting it as
  an unknown visual operator;
- imported library macro appears in autocomplete;
- accepting an unimported library autocomplete candidate adds the import;
- materialized source injects direct imports before patch body;
- transitive imports materialize in dependency order;
- import cycles produce a clear diagnostic;
- missing imports produce a clear diagnostic;
- local `defmacro` shadows a same-name library import deterministically;
- local shadowing applies to transitive dependencies deterministically;
- saving a local macro to library replaces it with `(use-defmacro name)`;
- forking a library macro creates a local `defmacro` and removes the direct
  import when appropriate;
- editing a local macro writes the current patch source/layout;
- editing a library macro writes the library source/layout;
- `instrument_probe` compiles a patch that uses an imported library macro.

## Implementation Phases

### Phase 1: Source And Materialization

- parse and preserve `(use-defmacro ...)` forms;
- add library package parsing and validation;
- add transitive dependency resolution with local shadowing and cycle
  diagnostics;
- route app compile/load, emitted source, and `instrument_probe` through the
  shared materializer.

### Phase 2: Autocomplete And Import Edits

- build the cached library index;
- show local/imported/library provenance in autocomplete;
- add `(use-defmacro name)` when accepting an unimported library candidate;
- validate materialized source after import edits.

### Phase 3: Library Save And Fork

- add `Save macro to library` for local macro views;
- add `Fork` for library macro views;
- move/copy macro layouts between current patch sidecar and library sidecar;
- make writeback route through explicit `PatcherViewSource` ownership.

### Phase 4: Polish And Watchers

- rebuild the index after library saves and file watcher events;
- surface shadowing explanations in UI diagnostics;
- add rename/overwrite flows for blocked save/fork cases.

## Implementation Notes

Do not implement this as text paste plus comments. The source needs explicit
semantic imports, and the writeback path needs explicit source ownership.

Do not make layout sidecars semantic. A patch copied without its layout file
should still declare all dependencies required for compilation.

Do not hide duplicate or shadowed definitions. Local definitions may win, but
the resolver should be able to explain why a particular macro definition was
chosen.
