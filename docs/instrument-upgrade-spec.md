# Upgrading a track to an instrument's current release

Status: rev 1, 2026-10-22. Extends `docs/instrument-versioning-spec.md`, whose
rev 1 listed "upgrade to latest" as a non-goal. Saves keep pinning `@<release>`
and nothing upgrades on its own. This spec adds an **explicit, reviewed,
undoable** upgrade that the user starts.

## Problem

Projects pinned to an old release (`factory:Synths/Digi Syn@1`) never pick up
the reworked instrument. The only way forward today is `swap-track-instrument`.
Swap resets every pattern to the new defaults and drops p-locks, key-locks,
neural overrides, macro mappings and the loaded preset
(`instrument_reset.rs:79`). For a finished part, that throws away the sound.

What the shipped releases look like (release 1 → current, by `(param …)` name):

| Lineage | Old → new | Same name | Notes |
|---|---|---|---|
| 808 Kick, 909 Kick, 808 Clap | 34/36/45 → same | all, same index | DSP retuned, layout identical |
| Digi Syn (from Digi Drift) | 62 → 81 | 62 | 19 new params; 33 ranges widened (dB floors −36 → −120, filter 20–18k → 10–20k) |
| PM Ride | 12 → 18 | 9 | `damping`/`touch`/`color` gone; `character` 0..1 → 0..5, `bell` 0..2 → 0..3 |

The flow has to handle all three cases: trivial, additive, and renamed or
repurposed.

## UX

### Badge

`instrument-header-row` (`content/ui/effects/instrument-panel.lisp:718`) shows a
small pill after the name when the track's release is below `current`:
`↑ Release 2`. The pill uses the accent colour and is clickable, and its
tooltip reads "Digi Syn release 2 is available". Rack-slot headers
(`:611`) get the same pill. The header ⋯ menu gains an "Upgrade to Release 2…"
item, so the badge is not the only way in.

The data comes from `instrument_release(name)` (`instrument_storage.rs:433`) via
the `inst` dict seed (`panel_kinds_seed.rs`): `release`, `current`, `lineage`
name. Unversioned and user-tier instruments never show the badge.

### Modal

Clicking the badge stages an **upgrade plan** in Rust and opens a modal that
reads it. This is the same shape as Factory Promote and Import Package: Rust
prepares, Lisp reviews, and a commit or cancel native finishes the flow. It is
one scrollable page with no wizard paging.

```
 Upgrade "Lead" — Digi Syn release 1 → 2                         [×]
 ─────────────────────────────────────────────────────────────────
 ✓ 62 parameters carry over                                 [show]
 ⚠ 4 values clamped to the new range                        [show]
     character   0.80  →  0.80   (range now 0–5, was 0–1)   ⚑ meaning may differ

 Needs a decision (3)
     damping  0.42     →  [ muffle         ▾]   suggested: rename in manifest
     touch    0.70     →  [ dynamics       ▾]
     color    0.30     →  [ drop           ▾]   (2 p-locks will be lost)

 New in release 2 (10) — start at default                    [show]
     voice_mode  Poly   ·  transpose  0 st  ·  …                each editable

 Also affects: 3 scenes, 2 takes, 1 sound in the pool, 14 p-locks
 ☐ Upgrade the other 2 tracks on Digi Syn release 1 the same way

                                        [Cancel]   [Upgrade]
```

- **Carried** rows are collapsed by default. Nothing needs doing for an
  identical layout (the drum case): the modal shows one green line and an
  Upgrade button.
- **Decision** rows list each old param with no confident match. The dropdown
  offers the new params no other row has claimed, plus `drop`. A manifest
  rename hint (below) pre-fills the choice. The default is `drop`, never a
  guess, and the row shows what dropping costs (p-locks, macro mappings,
  process bindings).
- **New** rows show the release-2 default as a small value field, so the user
  can set a starting value. The common case is leaving it alone.
- **⚑ meaning may differ** marks a same-name param whose kind or range changed
  shape: enum labels changed, or the range moved by more than ~2× without
  containing the old one. It is informational only.
- Upgrade commits as **one undo step**. A toast says "Upgraded Lead to Digi Syn
  release 2 — ⌘Z to revert". Undo is the audition mechanism; there is no
  separate preview mode.

## Matching

`plan_release_upgrade(old: &[ParamDescriptor], new: &[ParamDescriptor], hints)
-> UpgradePlan` is a pure function with no app access, so it can be
unit-tested against the real release pairs above.

Order of matching, each step only over params not yet claimed:

1. **Manifest hint**: `releases[n].upgrade.renames[old] = new` (below).
2. **Exact name** (`ParamDescriptor.name`).
3. **Persisted-name alias** via `resolve_persisted_param_name`
   (`effects/mod.rs:9031`), which already handles display-name aliases.
4. Nothing else. There is no fuzzy matching, and display-name or unit
   similarity only orders the dropdown.

The **value** carried is the stored (physical) value, never a normalized one.
A cutoff of 2500 Hz stays 2500 Hz even though the range widened.

- In range, compatible kind (`accepts_migrated_value_from`, `:585`): copy.
- Out of range: clamp and list it under "clamped".
- Enum → enum: match by **label**, falling back to the clamped index with ⚑.
- Boolean ↔ continuous, enum ↔ continuous: not compatible, so the row becomes
  a decision row, with the new default unless the user picks otherwise.

The same index map applies to defaults, p-locks and key-locks. P-lock values
go through the same clamp. Appended params (voice-modulator `mod{n}_*`,
modulation-target depths) are matched by name like any other. They shift
index whenever the DSP param count changes, so positional copy is never safe.

### Manifest hint

Instrument authors know about renames; the planner cannot. Each release can
carry optional upgrade notes from the release below it:

```json
"releases": {
  "1": { "path": "versions/1", "name": "Physical Models/PM Ride" },
  "2": { "path": ".", "name": "Physical Models/PM Ride",
         "upgrade": { "renames": { "damping": "muffle", "touch": "dynamics" },
                      "changed": ["character"] } }
}
```

- `renames` pre-fills decision rows. The user can still override each one.
- `changed` forces ⚑ on a same-name param whose meaning changed.

When several releases are skipped (1 → 3), hints compose in order. A missing
`upgrade` block is fine, because exact-name matching covers additive releases.

## Apply

One host command, `instrument-upgrade-commit`, runs inside
`App::apply_recorded_instrument_binding_mutation` (`app/edit.rs:1964`), so the
whole upgrade is one `EditPatch::InstrumentBinding` and rolls back on error.

1. Compile the target release's descriptor and rebind the track to its engine
   through the swap path (`swap_track_to_compiled_saved_instrument_sync`).
   Don't use `hot_reload_instrument`: engines are shared by name+source and
   hot reload would change every track on the engine.
2. **Before** the swap's reset clears anything, capture the old state and
   rewrite it through the plan's old→new index map:
   - every `Patch.instrument_slot` in the track's `TrackPatternPool` (scenes,
     takes and the track sound share the pool), the live slot, and orphan
     sounds: `defaults`, `plocks`, `key_locks`, `tensor_params` (by name).
     `plock_param_ids` / `key_lock_param_ids` are recomputed from the new
     descriptor (`ParamNodeId::from_slot_param`), because cell ids are
     release-specific;
   - neural `ProjectParamOverride` (`param_index` + `param_id`): remapped, or
     dropped with the param;
   - project macro mappings and rack macro targets: re-resolved by name,
     `param_id` refreshed, dropped only with their param;
   - process `bindings`, `fanout` and `ProjectSlotOverride` bindings: refresh
     `param_id` for all three. `rebind_track_process_chain_instrument_param_ids`
     (`runtime/process.rs:1145`) only rewrites `bindings` today, so fix that
     here;
   - plock variant registries: clear the instrument domains (they are derived).
3. Re-read the target release's `instrument.json`. Run mode and voice controls
   are per release and are not inherited.
4. `loaded_preset`: keep it if the new release's bank has a preset with the
   same name, otherwise clear it. The values stay either way; only the label
   is lost.
5. Republish rather than patch audio-thread state:
   `push_instrument_defaults_for_track`, epoch bumps and
   `publish_macro_overrides`, as swap does.

The track's id becomes `…@<current>` and the next save pins it.

**Undo hazard.** `restore_track_instrument_pattern_state` copies positionally
(`sync_to_descriptor_with_modulator`). That is correct only if undo restores the
old engine **before** the old slot snapshots. Write a test that upgrades,
undoes, and compares every stored pattern byte for byte.

## Scope

In:
- custom instrument tracks and rack slots
- factory-tier lineages
- the opt-in "same way for other tracks on this release" checkbox, which
  applies one plan to N tracks in one undo step

Out for rev 1:
- `.sound` / `.kit` files on disk (they stay pinned and load as before)
- user- and package-tier versioning
- auto-upgrade on load
- a project-wide "Upgrade outdated instruments…" list, which is the next slice
  once the per-track flow exists

## Slices

1. Planner: `UpgradePlan`, manifest `upgrade` hints, and tests against the real
   release pairs (808 Kick identical, Digi Syn additive, PM Ride renames).
2. Apply plus undo: remap all stored state for a custom track, the process
   fan-out `param_id` fix, and the byte-for-byte undo test.
3. Badge, staged plan native, and modal (Lisp), checked with `render-panel`.
4. Rack slots.
5. Multi-track checkbox, then the project-wide list.
