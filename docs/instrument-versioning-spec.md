# Factory instrument versioning

Status: rev 1, 2026-09-29. Epic: see `bd show` for the instrument-versioning epic.

## Problem

A factory instrument is identified by its folder path (`factory:Synths/Digi Drift`).
Projects and `.sound` files store parameter values positionally against that
folder's `dsp.lisp`. Reworking an instrument in place therefore changes the sound
of every saved project, and moving or renaming the folder makes those projects
fail to load (`projects.rs` aborts with "Could not resolve project instrument
id"). There is no alias, redirect, or version mechanism today.

We want to publish reworked instruments as a new release of the same instrument,
possibly under a new name (Digi Drift → Digi Syn), while every project saved
against an older release keeps loading and sounding exactly as it did.

## Non-goals

- **No "upgrade to latest".** Parameters change between releases, so remapping
  a slot from release N to N+1 is not attempted. Old projects stay on the
  release they were saved with.
- No versioning for user-tier or package-tier instruments yet. The id grammar
  allows it; only the factory tier ships manifests in this rev.
- No semver. Releases are integers, 1-based, monotonically increasing.

## Layout

The current release lives at the top of the instrument folder, exactly as today.
Retired releases live in a `versions/` subfolder, each a complete, frozen copy.

```
content/instruments/Synths/
  Digi Syn/
    instrument.json        manifest (below)
    dsp.lisp ui.lisp dsp.layout.json      current release (2)
    versions/
      1/                   frozen Digi Drift: dsp.lisp ui.lisp dsp.layout.json
      1.presets            release 1's bank
  Digi Syn.presets         current release's bank
```

Each release folder is a normal instrument folder. Once an id resolves to a
release folder, the existing sibling lookups (`ui.lisp`, `dsp.layout.json`,
`instrument.json`, `<folder>.presets`) work unchanged, which is what makes
presets version-specific for free: a bank belongs to exactly one release's
parameter layout.

A release folder may carry its own `instrument.json` (run mode, voice
controls). If it does not, it inherits none — the release is self-describing.

The browser already treats any directory containing `dsp.lisp` as a leaf and
does not descend, so `versions/` is invisible there. `list_saved_instruments`
does descend and must skip `versions/`.

## Manifest

`instrument.json` in the instrument's top folder gains two fields:

```json
{
  "version": 1,
  "run_mode": "instrument",
  "voice_controls": { "mode": "voice_mode", "count": "voice_count", "legato": "legato_on" },
  "current": 2,
  "releases": {
    "1": { "path": "versions/1", "name": "Synths/Digi Drift" },
    "2": { "path": ".",          "name": "Synths/Digi Syn" }
  }
}
```

- `version` stays the file-schema version (unchanged meaning; nothing reads it).
- `current` names the release new tracks get.
- `releases[n].path` is relative to the instrument folder.
- `releases[n].name` is the logical path that release shipped under. It is how
  unpinned legacy ids find their release (below). Two releases may share a name.

An instrument without `releases` is unversioned and behaves exactly as today.

## Ids

`ContentTier::parse_id` accepts an optional `@<release>` suffix on the logical
path: `factory:Synths/Digi Syn@2`.

Resolution of `factory:<logical>[@n]`:

1. **Pinned** (`@n`): find the manifest whose top folder is `<logical>`, take
   `releases[n].path`. Unknown `n` is a load error naming the id and the
   releases that exist.
2. **Unpinned**: if `<logical>` is itself an instrument folder with a manifest,
   or matches some release's `name`, resolve to the **lowest** release whose
   `name` equals `<logical>`. Unpinned ids only exist in files saved before
   pinning shipped, so the lowest release with that name is the one they were
   saved against. `factory:Synths/Digi Drift` → release 1;
   `factory:Synths/Digi Syn` → release 2.
3. Otherwise fall through to today's resolution (unversioned instruments).

The name → release index is built once by scanning factory manifests and cached
alongside `resolved_walk_cache`.

**Saves always pin.** `qualify_instrument_id` emits `@<release>` for any
instrument with a manifest, so a file saved today names the exact release it
was authored against and survives any later release. Unversioned instruments
save unchanged.

New tracks created from the browser use `current`.

## Presets

- Factory bank: `<release folder>.presets` beside the release folder
  (`versions/1.presets`, `Digi Syn.presets`). No resolver change beyond
  resolving the folder.
- The preset list in the UI shows the bank of the slot's release, so an old
  project's track shows release 1's presets.
- User overlays are keyed by the release's logical path. Existing
  `~/.eseq.d/instruments/Synths/Digi Drift.presets` must keep attaching to
  release 1: the overlay lookup tries the release's `name` path as well as its
  folder path.

## Shipping a new release

1. `git mv` the top-level release files into `versions/<current>/` and the bank
   to `versions/<current>.presets`.
2. Point `releases[<current>].path` at `versions/<current>`.
3. Add the new files at the top, add `releases[<current+1>]` with `path: "."`,
   bump `current`. Rename the top folder if the name changes.

Never edit a file under `versions/` after it ships.

## First application: Digi Drift → Digi Syn

- Move `Synths/Digi Drift/{dsp.lisp,ui.lisp,dsp.layout.json}` to
  `Synths/Digi Syn/versions/1/`, and `Digi Drift.presets` to
  `Synths/Digi Syn/versions/1.presets` (fix its `engine_name`/`source_file`).
- Write the manifest above.
- Factory content that referenced Digi Drift (Loneristic Lead/Chorder sounds,
  Melt Again kit) was removed rather than migrated.
- Rust tests and capture fixtures that load Digi Drift by name keep working
  through unpinned resolution; they are the regression coverage for it.
  The manual text should say Digi Syn.
- `drift-waveform` widget math is dual-maintained with the Digi Drift DSP; it
  must keep matching whichever release's `ui.lisp` uses it.

## Caches

- The dylib cache keys on source content, so moving files costs no recompile.
- The engine registry keys on name + source; two releases differ in both.
- Instrument favorites key on canonical id; a favorite of `Synths/Digi Drift`
  should canonicalize to the lineage's current release so it survives renames.
