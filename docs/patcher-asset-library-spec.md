# Shared Asset Library for DGenLisp `@file` References

Rev 1 — 2026-08-31

## Motivation

Building a wavetable synth in the patch editor requires a file-backed tensor:
`(tensor @shape [512 448] @file "waves/bank.json")`. Today a relative `@file`
reference resolves against the instrument's own directory (`asset_base` =
parent of `dsp.lisp`), which makes fresh patcher drafts painful:

- A new instrument draft lives in `$TMPDIR/eseq-instrument-drafts/draft-<pid>-<stamp>/`
  (`edit_sessions.rs::create_new_instrument_draft_dir`). Nothing in the UI
  exposes that path, so there is no reasonable way to place an asset next to
  an unsaved draft.
- Cross-instrument references like `@file "../core/wavetable/waves/bank.json"`
  do not resolve in a draft at all (the temp dir has no siblings), and after
  save they couple one instrument to another's location: rename or delete the
  referenced instrument and the referencing one silently breaks. Fork and save
  asset copying (`patch_fork::fork_patch_files`,
  `patch_fork::materialize_forked_assets`) only ever copy the instrument's own
  directory, by design.
- Inlining a large bank via `@data` is a non-starter (the core wavetable bank
  is 448 waves x 512 samples).

The fix is a **shared asset library root** with a resolution fallback, plus UI
to browse and insert those assets. An instrument then writes
`@file "wavetables/basic-shapes.json"` and it resolves identically in a temp
draft, a saved instrument, and a fork — no copying, no `../`.

## Design overview

1. **Library roots** — two-tier, following the existing content/app_paths
   pattern (factory vs user):
   - Factory: `content/assets/` in dev; the bundled equivalent in release.
   - User: an `assets/` directory under Application Support, created on
     demand (sibling of `user_instruments_dir()` / `user_effects_dir()`).
   Initial curated layout: `assets/wavetables/*.json`. The layout is flat and
   convention-only; resolution treats the root as opaque.
2. **Resolution fallback** — a relative `@file`/`@default-file` reference is
   tried in order: `asset_base` (instrument dir / draft dir), then the user
   assets root, then the factory assets root. First hit wins. No hit is a
   compile error naming the reference and all three roots.
3. **UI** — an Assets section in the patcher sidebar (drag-drop creates a
   ready-made `tensor` node), `@file` path autocomplete in the node editor,
   attribute-key autocomplete, and a file-drop import into the draft for
   one-off assets.

## Resolution mechanics (the key implementation fact)

The DGenLisp compiler is an external lock-pinned binary; the host passes one
`--asset-base` (`effect_compile.rs::compile_effective_dgen_source_to_dir`).
Inside the compiler, `LispEvaluator` resolves references with
`URL(fileURLWithPath: file, relativeTo: sourceDirectory)` — Foundation
semantics: **an absolute `file` ignores `relativeTo` and passes through**.

Therefore the fallback lives entirely host-side, with **no dgen change and no
dgenlisp republish**:

- Before invoking the compiler, scan the effective source's asset references
  (`dylib_cache::asset_references` already tokenizes exactly
  `@file`/`@default-file` string values). For each relative reference that
  does **not** exist under `asset_base`, if it exists under a library root,
  rewrite that reference **in the effective source only** to the resolved
  absolute path. If it exists nowhere, fail the compile with an error naming
  the reference and the searched roots (same hard-fail style as the toolchain
  checks).
- The **authored source is never rewritten**. `dsp.lisp` on disk, the patcher
  model, and writeback all keep the library-relative spelling. The rewrite
  happens at the same seam that already produces the effective source for the
  compiler subprocess.
- **Cache-key parity**: `dylib_cache.rs::~1104` fingerprints asset content by
  resolving references against `asset_base` (or
  `app_paths::dgen_asset_fallback_base()` when absent). The fingerprint
  resolution must mirror the new fallback order exactly, so editing a library
  asset busts the dylib cache, and so two instruments referencing the same
  library asset share resolution semantics with the compile itself. Factor
  one `resolve_asset_reference(reference, asset_base) -> Result<PathBuf>`
  helper and use it from both the fingerprint path and the rewrite path.

Long-term option (not required by this epic): teach DGenLisp a repeatable
`--asset-search-path` flag and drop the rewrite. That is a cross-repo change
gated on a dgenlisp release + lock bump; the host-side rewrite is invisible to
authored content, so migrating later is free.

### Non-goals / invariants

- `materialize_forked_assets` and `fork_patch_files` stay untouched: library
  references are deliberately **not** copied on fork or save. An instrument's
  own `waves/` assets keep working exactly as today (asset_base wins first).
- Absolute `@file` paths keep passing through untouched (current behavior).
- No change to `@data`, `tensor-param` seeding, or the preamble helpers
  (`wavetable-read` / `wavetable-morph` / `sample`).

## UI slices

### Assets sidebar section

`content/ui/patch-macros.lisp` already renders the patcher sidebar as a tree
whose Library rows drag into the patch as `"dgen-macro"` items, with the drop
handler creating a node at the drop point. Add an **Assets** section (or tab)
sourced from: the current draft/instrument dir's assets, the user assets
root, and the factory assets root — labeled by tier. Rows use a new drag type
(`"dgen-asset"`); dropping one creates a fully-formed node:

```
tensor @shape [512 448] @file "wavetables/basic-shapes.json"
```

with `@shape` inferred by reading the JSON at drop time (factory.json-style
dict; same format `gen_bank.py` emits and the `wavetable-viewer` widget
loads). Double-click or a secondary affordance can later preview the asset
(non-goal for the first slice).

### `@file` path autocomplete

The patcher node editor's autocomplete
(`patcher/text.rs::patcher_autocomplete_suggestions`) currently fires only on
the **first token** (`autocomplete_prefix` bails once whitespace follows it).
Add a completion context: when the cursor is in the value position after
`@file`/`@default-file`, complete file paths from the same three sources the
sidebar lists — draft-relative paths for draft assets, library-relative paths
for library assets. Reuse the existing popup/selection/tab-apply machinery
and the attribute-span helpers in `patcher/lisp.rs` (never hand-roll
`idx += 2` past an `@attr` — see `attribute_span_len`).

### Attribute-key autocomplete + ghost text

When the cursor is in a `@`-prefixed token, complete attribute keys from the
manifest's per-operator `attributes` list
(`crates/sequencer/tools/dgenlisp-operators.json`; e.g. `tensor` carries
exactly `@data @file @name @shape`), filtered by the node's op. Render the
selected match's remainder as gray inline ghost text after the cursor; Tab
accepts. This slice is independent of the library root.

### Add-asset-to-draft (escape hatch)

OS file-drop onto the patcher canvas copies the file into the draft/instrument
directory (under `waves/` by convention) and inserts a ready-made `tensor`
node with a draft-relative `@file`. This is the path for one-off custom banks
that don't belong in the shared library. Save already carries these along via
`materialize_forked_assets`.

## Packaging / release notes

- The factory assets root must ship in the release bundle (add to the
  packaging manifest next to the other content dirs) and be registered in
  `app_paths` for both Dev and Release, including the release-layout
  assertion list (`app_paths/mod.rs::~997`).
- Seed the factory library with at least one bank so the sidebar is never
  empty — e.g. generate a starter `wavetables/basic-shapes.json` (a small
  set, not a copy of the full 448-wave core bank).

## Gotchas recorded during design

- Draft dirs are host-minted temp dirs; the patcher widget knows the dsp.lisp
  path via its `:path` prop, so both autocomplete and the sidebar can derive
  `asset_base` without new plumbing.
- Patcher attributes on builtin nodes ride in `node.label` (bracket-array
  handling fixed 2026-08-08); generated nodes must go through
  `normalize_editor_node_text` seams, not ad-hoc string joins.
- `asset_references` hard-errors on a non-string after `@file`; the rewrite
  pass inherits that contract (fine — the compiler would reject it anyway).
- Fingerprint/rewrite divergence is the one silent-corruption risk: if the
  cache fingerprints one resolution and the compiler reads another, a stale
  dylib can serve edited asset content. Hence the single shared resolver
  helper and a test that compiles, edits the library asset, and asserts a
  recompile.
