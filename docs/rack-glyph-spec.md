# Rack Glyph Spec — slot-per-lobe composite glyphs for rack tracks

Status: PLANNED (rev 1)
Related: `docs/sound-glyph-spec.md`, `docs/delta-glyph-spec.md`

## 1. Problem

Rack tracks (Layer Rack, Drum Rack, group-to-rack) render **empty glyphs** in
the sound palette and the mixer pattern-cell grid. Three independent
collapses, all in the glyph feed (`src/ui/state_values/sound_palette.rs`):

1. **Schema**: a rack track's instrument descriptor is
   `EffectDescriptor::empty_custom_slot()`
   (`finish_rack_track_registration`, `src/app/graph/node_build.rs:342`), so
   `glyph_schema_for_descriptor` yields zero `ParamSchema` entries — no
   substrate slots, no accent pieces.
2. **Identity tier**: `ensure_identity_cached` finds neither a custom
   instrument name nor a track engine (`track_engine_ids[track]` is `None`
   for racks), falls back to `stock_skeleton(empty descriptor)` → zero
   branches → no silhouette. The delta-glyph "never empty" guarantee assumes
   at least one branch or param; racks violate the assumption upstream.
3. **Values**: `patch_glyph_values` reads `patch.instrument_slot.defaults`,
   which is empty for rack patches.

The data exists: `Patch.rack_track: Option<RackTrackSnapshot>`
(`src/sequencer/state/sound_entities.rs:61`) carries every slot's
`instrument_slot` values, instrument type/engine binding, gain/pan/mute, the
per-slot fx chains, and the rack macros.

## 2. Design: slot = lobe

One glyph per rack patch, built by the existing delta-glyph pipeline, where
**each rack slot claims one spatial lobe** (one `ParamGroup`) and the rack
macros + slot mix form the played-surface group. The glyph reads as "a rack
of N sounds"; per-lobe density tracks that slot's settings; patch-to-patch
diffs light pieces in the lobe of the slot that changed.

Why not a raw param-soup merge: interleaving 3–4 instruments' osc/filter/env
groups produces indistinguishable mush, and the ink cap (`MAX_LIT`,
`src/delta_glyph.rs`) would saturate on nearly every rack diff. Forcing one
group per slot keeps localization meaningful and lets the existing top-K
piece selection spread across lobes.

### 2.1 Composite schema

New helper next to `glyph_schema_for_descriptor` (same file), used by both
feeds:

```
fn rack_glyph_schema(app, rack: &RackTrackSnapshot) -> Vec<ParamSchema>
```

For each slot `i`:

- Resolve the slot descriptor with `app.rack_slot_instrument_descriptor(slot)`
  (`src/app/synth.rs:685` — sampler → builtin sampler descriptor, custom →
  engine registry). A slot whose engine isn't registered contributes only its
  mix params (see below) so the schema never blocks on a disk load.
- Call `glyph_schema_for_descriptor(desc, &format!("slot{i}:"), offset,
  Some(ParamGroup::Other(format!("slot{i}"))))` — the three hooks exist for
  exactly this. `offset` accumulates so `order` stays globally unique.
- Append synthesized `ParamSchema` entries for the slot's mix surface —
  `slot{i}:gain` (continuous), `slot{i}:pan` (continuous), `slot{i}:mute`
  (boolean) — in the same forced group. These are what players actually
  tweak between layered-rack patches; they must diff.
- The existing hidden-param filter (mod plumbing, `hidden`/`ui`/`non-audio`
  tags) applies per slot for free inside `glyph_schema_for_descriptor`.

Then the rack surface:

- One entry per rack macro (`rack.macros`, `RackMacro.value`), ids
  `macro:{index}`, group `ParamGroup::Mod`, **weight 1.75** (schema `weight`
  field, currently always 1.0): macros are the rack's played surface and
  should win piece selection against deep slot params at equal normalized
  delta.

### 2.2 Composite values

```
fn rack_glyph_values(rack: &RackTrackSnapshot, schema_slots: ...) -> Vec<f32>
```

Mirrors schema order exactly: per slot — `slot.instrument_slot.defaults`
(padded with descriptor defaults, same rule as `patch_glyph_values`), then
`gain`, `pan`, `mute as 0/1` — then macro values. Source is
`patch.rack_track` for pool patches; a rack patch with `rack_track == None`
is incompatible (renders the ringed bare grid, existing mechanism).

### 2.3 Identity tier: grafted per-slot skeletons

For rack tracks, `ensure_identity_cached` builds the skeleton by grafting:

- Per slot, extract a child skeleton:
  - Custom slot → engine source from
    `engine_registry.get(slot.track_sound_state.engine_id)` →
    `extract_skeleton(&source)`; fall back to
    `load_instrument_source(name)` like the flat-track path.
  - Sampler slot → `stock_skeleton(builtin_sampler())`.
- Graft as one root `Branch` per slot: `cluster = format!("slot{i}")`,
  `children` = the slot skeleton's top-level branches, `weight` = sum of
  child weights (min 1).
- Add a `macros` root branch, weight = macro count.

`identity_branches` (`src/sound_glyph/mod.rs:36`) then does the right thing
unmodified: with ≤ 6 root branches it's under `THIN` and expands into
`slot{i}/{cluster}` children — per-slot sub-lobes at low resolution. An
empty rack still yields the `macros` branch, restoring the never-empty
guarantee.

**Cache key**: `identity_cache_key` gains a rack arm. The flat-track probe
(custom name, param count, engine id, registry epoch) never fires for racks.
Rack key ingredients: registry epoch + a **rack glyph signature** — hash of
(slot count, per-slot `instrument_type`, per-slot
`track_sound_state.engine_id`, per-slot descriptor param count, macro
count). This is deliberately looser than `rack_topology_signature`
(`src/app/graph/mod.rs:61`), which includes fx-chain node ids that churn on
graph rebuilds without changing the glyph.

### 2.4 Compatibility + fingerprints

- `compatible(patch)` for rack tracks: replace the `defaults.len() ==
  descriptor.params.len()` check with "patch's `rack_track` glyph signature
  == the track's live rack glyph signature". Slot added/removed/swapped ⇒
  structural mismatch ⇒ the existing incompatible-ring treatment, exactly
  like a flat track whose instrument was swapped. Muted slots stay in the
  schema and identity (the rack *is* those layers); the mute itself is a
  boolean param, so toggling it lights one piece without reshaping anything.
- `cached_descriptor_glyph_hash` probe is insufficient for racks (rack edits
  bump neither engine id nor descriptor name/count). Rack arm: probe on
  (registry epoch, rack glyph signature); on miss, hash the full composed
  schema inputs with the existing `hash_descriptor_glyph_inputs` shape
  applied to the composite.
- The live rack snapshot both feeds diff against comes from the same source
  the rack UI reads (the track's current pattern-state rack snapshot), NOT
  from disk or a pool entity, mirroring the "glyph shows what's COMPILED"
  rule.

## 3. Build plan

**Phase 1 — surface seam (behavior-identical refactor).** Both feeds
(`sync_glyph_frames`, `sync_pattern_cell_glyph_frames`) resolve
descriptor/schema/identity/values through one per-track resolver struct
(`GlyphSurface`: schema, identity branches, value-extractor, compat
predicate, fingerprint hash). Non-rack fingerprints must not change (assert
via existing tests). This is where the rack arm plugs in without duplicating
logic across the two feeds.

**Phase 2 — composite schema + values.** `rack_glyph_schema` /
`rack_glyph_values` + rack compat predicate + rack fingerprint arm. Glyphs
appear (accent pieces + substrate) but identity silhouette still generic.

**Phase 3 — grafted identity.** Per-slot skeleton grafting + rack identity
cache key. Silhouettes differentiate racks with different slot lineups.

**Phase 4 — tuning + QA.** Macro weight (start 1.75), check ink-cap behavior
on a real 3–4 slot Layer Rack with two diverged patches; verify Drum Rack
degradation (see §5); screenshot pass against the mixer clip grid.

## 4. Tests

Follow the scoped-test workflow (`cargo test -p sequencer <filter>`; never
package-wide fmt; worktrees not stash).

- Schema composition: 2-slot rack → slot-prefixed ids, forced
  `Other("slot{i}")` groups, contiguous order offsets, mix params present,
  macro entries in `Mod` with weight 1.75, hidden mod-plumbing filtered.
- Values: defaults padding, mute as 0/1, macro values, ordering matches
  schema; `rack_track == None` ⇒ incompatible.
- Identity: 2-slot custom+sampler rack grafts to ≥ 2 root branches;
  `identity_branches` expansion yields `slot{i}/…` names; empty rack yields
  the `macros` branch (never empty); cache invalidates on slot engine swap
  but NOT on unrelated registry-epoch-only churn for flat tracks (existing
  behavior untouched).
- Compatibility: patch with 3-slot snapshot against 4-slot live rack ⇒
  incompatible ring path (mirror the existing incompatible-patch tests).
- Feed integration: extend the layer-rack action tests in
  `src/ui/state_values/tests.rs` (~4117) — after adding a layer rack and a
  slot, `sync_glyph_frames` publishes a frame with non-empty substrate.

## 5. Open questions / deferred

- **Drum Rack v1 scale**: 16+ pad slots → 16+ lobes exceeds `MAX_BRANCHES`
  merging comfort and schema size. v1 ships the same path (correctness over
  beauty); if it reads as noise, collapse drum-rack slots to mix-params-only
  (gain/pan/mute per pad) + identity from pad sample-name hashes. Layer Rack
  is the priority (the empty glyphs in the report are Layer Racks).
- **Drum Rack v2 (group over real tracks)**: child tracks own real glyphs;
  the group's glyph should eventually derive from child identities so both
  rack flavors stay visually coherent. Out of scope here.
- **Per-slot ink budget**: if one wildly-diverged slot starves the others'
  pieces under `MAX_LIT`, consider a per-group piece cap in the cohort
  model. Only if Phase 4 QA shows it.
- **Nested racks**: `InstrumentType::Rack` inside a slot resolves no
  descriptor (`rack_slot_instrument_descriptor` → `None`); it contributes
  mix params only. Fine for v1; note it.
