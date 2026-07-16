# Spec: Ableton-style instrument swap (rev 1)

Replace the instrument on an *existing* track — by dragging an instrument from
the browser onto the track, or by double-clicking an instrument in the
Instruments tab — instead of today's only option of adding a whole new track
per instrument. This is the workflow every producer knows from Ableton: punch
in a pattern, then audition Operator → Sampler → Drift on the same notes until
one sticks.

This is deliberately the *first step* of the larger instrument-flexibility
arc. It builds exactly the machinery later steps need (descriptor resync +
p-lock clearing across all pattern snapshots, engine rebinding, track-type
conversion) without touching the serialization format at all. Follow-ons, out
of scope here:

- **Sounds tab**: instrument-agnostic "sound" entries (instrument + preset
  pairs) swappable into a track over and over. Falls out of this spec's swap
  command + the existing preset system; sketched in "Future" below.
- **Empty (instrument-less) tracks** and **per-pattern instruments**
  (Autechre-style slot changes across scenes). The rack subsystem already
  proves the per-pattern graph-rebuild approach (`sync_live_rack_tracks_from_pattern_state`,
  `src/ttui/graph.rs`); not touched here.

## UX behavior

### 1. Drag instrument → existing track

Instruments in the browser tree already carry `:drag-type "instrument"`
(`ui/browser.lisp`, `sbrowser-create-picker` ~line 684), and rack
panels/drum pads already accept them. New drop targets, all with
`:drop-types (list "instrument")`:

- **Track rows** in the sequencer track list (the same rows that accept
  track-group drag-drop today, `metal_seq/main.rs` ~line 9691).
- **The instrument panel** of the focused track (`instrument-panel.lisp` /
  `panel-frame.lisp` header) — dropping anywhere on the device panel replaces
  that track's instrument.
- **Mixer strips** (same handler, `:track` in drop-meta).

Drop semantics by target track type:

| Target track  | Dropped payload      | Result                                   |
|---------------|----------------------|------------------------------------------|
| Custom        | saved instrument     | swap engine, same track (Phase 1)        |
| Sampler       | saved instrument     | convert track to Custom + swap (Phase 2) |
| Custom        | sample (`"sample"`)  | convert track to Sampler (Phase 2)       |
| Rack          | instrument           | unchanged — existing rack-slot drop wins |
| Modulator     | instrument           | rejected with status message             |

Builtin-instrument leaves (`sampler`, `modulator`, `rack`, `layer-rack`) are
currently `draggable: false` (`metal_seq/browser.rs:600`). Phase 2 makes
`sampler` draggable (drop = convert to sampler track, keeps current sample if
the track ever had one, else opens sample browser context). `modulator` /
`rack` / `layer-rack` stay non-draggable — converting into those is a
different feature.

While hovering a valid target, reuse the existing `drop-hover-border-color`
affordance (see drum pads, `instrument-panel.lisp:100`).

### 2. Double-click in the Instruments tab

`sbrowser-select-create-item` (`ui/browser.lisp:627`) is the
`:on-activate` handler (double-click / Enter). **Behavior change:**

- Today: activate always **adds a new track** (`add-track-instrument`).
- New: activate **replaces the current track's instrument** (Ableton
  behavior), using `SEQ.current-track`.
  - Keep the Instruments tab active after activation so consecutive
    double-clicks can audition different instruments quickly.
  - If the project has zero tracks, or the current track is a Rack/Modulator,
    fall back to add-new-track (status line says which happened).
  - Builtin leaves: `sampler` converts the current track (Phase 2); until
    then builtin activation keeps adding a new track.

New-track creation remains available: drag an instrument to the empty area
below the track list (new drop zone, Phase 1), plus the existing Ctrl+N
instrument-picker flow. If losing "double-click = new track" proves painful,
a modifier (Cmd+double-click = add track) is the escape hatch — decide after
living with it.

### 3. Feedback

- Reuse `sbrowser-loading-instrument-name` for the compile-in-progress row.
- Status line on completion: `"Swapped → drums/ultrakick"`, or the reason a
  drop was rejected.
- Swap is **destructive to instrument-specific data** (see below). No
  confirmation dialog — the whole point is fast auditioning — but the status
  line should say when p-locks were cleared: `"Swapped → wavetable (cleared
  instrument p-locks in 3 patterns)"`.

## What survives a swap, what's cleared

Preserved (the "musical" layer — this is why swapping beats re-adding a track):

- Step/trigger data, note step-params, chords, timebase/swing (incl. their
  p-locks), track params (volume, pan, sends, output, polyphony, mute groups,
  midi-fx chain, …)
- Track FX chain + effect p-locks, MIDI FX
- Process chains, *except* bindings that target instrument params (below)
- Track color/group membership, mod connections (they target mod-in
  clips, which are per-track and instrument-agnostic)
- `instrument_base_note_offset` (it's effectively track transpose)

Updated:

- Track display name follows the new instrument's leaf name so track badges
  identify the instrument currently bound to the track

Cleared / reset, in **every pattern**, not just the current one (the
per-pattern `instrument_slot` layout *must* match the new descriptor — this is
the same invariant `migrate_project_instrument_slots` exists to maintain):

- `instrument_slot`: defaults ← new descriptor defaults; all instrument
  p-locks and key-locks dropped; tensor params dropped
- `track_sound_state`: `loaded_preset = None`, `dirty = false`, `engine_id` →
  new engine
- `instrument_type` / `instrument_run_mode` per pattern (run mode ← the new
  instrument's declared mode, `lisp_host::load_instrument_run_mode`)
- Instrument-scoped entries in `plock_variant_registries` /
  `key_lock_variant_registries` (verify scoping — if a registry mixes
  instrument and effect entries, clear only the instrument ones)
- Macro mappings whose `ParamTarget` points at the swapped track's instrument
  params (branch `codex/macro-mapping`); effect-param targets survive
- Neural `output_overrides.instrument` entries targeting this track
- Process-chain effect/instrument bindings that resolve to instrument params
  (`refresh_track_process_chain_binding_param_ids` already re-resolves; stale
  ones must drop, not mis-bind)

## Engine + graph mechanics (Rust core)

The key architectural fact: a swap is a **rebind, not a reload**.
`hot_reload_instrument` (`src/ttui/graph.rs:1985`) mutates the *engine* in
place, which would clobber every other track sharing that engine. Swap instead
changes *which engine the track's routes point at* — exactly what the rack
rebuild already does per pattern:

```
App::swap_track_instrument(track, name) -> Result<(), String>
```

1. **Resolve the new engine.** Same pipeline as `add-track-instrument`
   (`metal_seq/main.rs:7015`): cache hit via
   `cached_instrument_engine_idx(name, source)` → sync swap; miss → the
   existing background-compile path (`pending_saved_instrument_load`
   analogue: `pending_instrument_swap { track, name }`), then a
   `apply_compiled_instrument_swap` on completion. FreePatch-mode instruments
   get a dedicated engine via `register_dedicated_instrument_engine`, as in
   `try_add_cached_saved_instrument_track_sync` (`src/tui/effects.rs:442`).
2. **Rebind the graph** under a `GraphEditBatchGuard`:
   - `delete_engine_route_for_track(old_engine_id, track)` (exists, used by
     rack rebuild `src/ttui/graph.rs:3628`)
   - `ensure_custom_engine_runtime(new_engine_id, …)` — lazily materializes
     the engine's synth/modulator nodes if not yet in the graph (exists)
   - `connect_engine_to_track(new_engine_id, track, …)` (`src/ttui/graph.rs:4697`)
     — per-voice route gains into the track's existing `voice_sum` /
     `voice_sum_r`, so the whole downstream chain (FX, mixer, sends) is
     untouched by construction
   - Update `graph.track_engine_ids[track]`, `graph.instrument_descriptors[track]`
     via `apply_instrument_slot_descriptor(track, name, manifest,
     preserve_runtime_values: false)` (`src/ttui/graph.rs:5476` — the reset
     branch already exists), `self.tracks[track] = instrument_display_name(name)`
   - FreePatch: `apply_free_patch_idle_voice(track)` as hot-reload does
3. **Reset pattern state across all scenes.** For the live pattern: reset the
   `instrument_slot` in `state.pattern.instrument_slots[track]` and push new
   defaults to the audio thread (`push_instrument_defaults_for_track`,
   `src/tui/synth.rs:1602`). For every saved scene snapshot
   (`pattern.scenes`): rewrite that track's lane (`TrackPatternData`,
   `state.rs:498`) with the cleared fields from the table above. Add a
   `SequencerState` helper (`reset_instrument_slot_all_patterns(track, desc)`)
   next to `sync_instrument_slot` (`state.rs:2381`).
4. **Old-engine GC / keep-warm.** If no track and no rack slot references the
   old engine, *keep the engine registered and its dylib loaded* (that's the
   pre-warm that makes A/B-ing instruments instant) but disconnect/remove its
   graph nodes if `engine_node_ids` reports no remaining routes. A projectwide
   cap or project-save-time sweep can come later; memory cost is per distinct
   instrument, which is the same order as today's one-track-per-instrument
   workflow.
5. **Voice safety.** Kill/steal active voices on the track before rebinding
   (same guard the rack rebuild uses); do the rebind inside the batch guard so
   the audio thread never sees a half-wired track.

### Track-type conversion (Phase 2)

Sampler → Custom and Custom → Sampler reuse the same skeleton; the
instrument-side teardown/build differs:

- Sampler → Custom: tear down the track's sampler voice pool nodes, then step
  2 above. Clear `track_buffer_ids[track]` / sample metadata per pattern
  (`sample_ids` in `TrackPatternData`).
- Custom → Sampler (drop a sample on a custom track): route teardown, then
  build sampler voices into the same `voice_sum` (exactly what
  `build_sampler_voices` does for rack slots, `src/ttui/graph.rs:3677`), load
  the dropped sample, set `sample_ids` in all patterns.
- Update `track_instrument_types[track]`, per-pattern `instrument_types`, and
  the `ProjectTrack` enum variant (happens automatically at save time — see
  Serialization).

### Serialization

**No format change.** `ProjectTrack::Custom { instrument_name }`
(`src/project.rs:402`) picks up the new name at save; per-pattern
`instrument_slots` / `instrument_types` / `track_sound_states` are already
per-pattern vectors that we're rewriting in place. Old projects load
unchanged. (This is the big reason to ship swap before per-pattern
instruments, which *does* need a format change.)

## Host commands + Lisp wiring

New host commands (handled in `metal_seq/main.rs`'s command match, or
`host_commands.rs` if that registry fits better):

- `swap-track-instrument` `(dict :track N :name "drums/ultrakick")`
- `swap-track-builtin-instrument` `(dict :track N :name "sampler")` (Phase 2)
- `convert-track-to-sampler` `(dict :track N :path "...")` (Phase 2 — the
  sample-onto-custom-track drop)

Lisp side:

- `ui/browser.lisp`: `sbrowser-select-create-item` branches on
  "instrument" → `swap-track-instrument` with `SEQ.current-track` (fallback to
  `add-track-instrument` when no tracks / non-swappable track type).
- `drag-drop.lisp` (or a new `instrument-swap.lisp` beside it): drop handlers
  for track rows, instrument panel, mixer strips, "empty area = new track"
  zone, mirroring `fx-drop-library-effect`'s shape.
- `instrument-panel.lisp`: add `"instrument"` to the sampler panel's
  drop-types (Phase 2 conversion) — it already takes `"sample"`.

## Testing

- **Rust unit tests** (pattern-state layer): swap on a 3-pattern project
  clears instrument p-locks/key-locks and resets slot defaults in *all*
  patterns while step data, effect slots + their p-locks, and track params
  survive; `instrument_types` consistent across live state + snapshots;
  project save/load roundtrip after swap; swap on a track sharing an engine
  with another track leaves the other track's binding and slot untouched.
- **Graph tests**: after swap, old engine has zero routes for the track, new
  engine has `MAX_VOICES` routes into the same `voice_sum` ids; engine with no
  remaining references keeps its registry entry (keep-warm) but no dangling
  nodes.
- **Lisp UI layout tests** (use the UI-script test pattern; `each` not `map`
  for generated children): activation swaps instead of adding; drop-target
  metadata present on track rows/panels; loading row shows during async
  compile.
- **Audition harness** (`tools/audition/audition.py`): after swap, the track
  actually sounds like the new instrument (render a bar, compare against the
  instrument auditioned standalone).

## Phases

1. **Custom→Custom swap**: core `swap_track_instrument`, cross-pattern reset,
   host command, double-click activation, drag onto track rows + instrument
   panel, empty-area-drop = new track.
2. **Type conversions**: sampler↔custom, builtin `sampler` draggable,
   sample-onto-custom-track, macro/neural/process cleanup edge cases.
3. **Future — Sounds tab**: browser tab listing (instrument, preset) pairs;
   activating one = `swap-track-instrument` + `load-instrument-preset`
   (`ui/browser.lisp:442`). Storage: a `sounds/` index of
   `{instrument, preset-name}` entries; instrument-agnostic by construction
   once swap exists. The richest Sound is a **rack preset presenting a rack
   macro bank**: the rack loads collapsed, showing only its 6–8 macros
   (Ableton's model). Design locked in `docs/racks-spec.md` A3 #6/#7 —
   rack-relative macro addressing serialized with the rack is precisely what
   makes such a Sound portable across projects and tracks.

## Open questions

- **Undo**: swap destroys instrument p-locks across patterns. If an undo stack
  exists for pattern edits, swap should push a snapshot; if not, this is the
  strongest argument for adding one, but it's out of scope here (status-line
  transparency is the interim mitigation).
- **Double-click regression risk**: is anyone (muscle memory, docs, agent
  scripts) depending on activate-adds-track? Revisit after Phase 1 dogfooding.
- **Keep-warm cap**: unbounded engine retention is fine until it isn't;
  decide a policy (LRU cap / sweep on save) when a real project hits it.
