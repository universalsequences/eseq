# Custom Engine + Preset System Spec

## Goal

Replace the current "one custom instrument instance per track" model with a shared engine model:

- a loaded custom instrument is an `Engine`
- an engine owns a shared pool of voices
- tracks reference an engine
- each track owns its own editable sound state derived from a preset
- presets are stored per engine
- pattern switching restores the track's sound state for that pattern

This is intended to behave more like Elektron synth machines: shared synthesis engine, track-owned sound config, shared preset library.

## Terminology

### Engine

A compiled DGenLisp synth definition plus its shared runtime resources.

Fields:

- `engine_id`
- `name`
- `source_path`
- `source_hash`
- `manifest`
- `loaded_lib`
- `voice_count`
- `param_schema`
- `preset_index`
- `voice_pool`

### Preset

A named snapshot of parameter values for a single engine.

Fields:

- `preset_id`
- `engine_id`
- `name`
- `param_values`
- `base_note_offset`
- `created_at`
- `updated_at`
- optional `tags`

### Track Sound

A track-owned editable copy of a preset.

Fields:

- `engine_id`
- `loaded_preset_id: Option<PresetId>`
- `param_defaults`
- `base_note_offset`
- `dirty_from_loaded_preset`
- optional `working_name`

This is the state the user edits in the Synth tab.

### Voice Fingerprint

A stable identifier for the effective sound config that a voice is currently loaded with.

Input data:

- `engine_id`
- full `param_defaults`
- `base_note_offset`

This should not depend on track id. Two tracks with identical effective sound state should produce the same fingerprint.

## Product Rules

### Engine Loading

When the user loads a custom instrument source:

1. Compute a stable engine key from the source file path and source hash.
2. If an engine with the same key is already loaded, reuse it.
3. Otherwise:
   - compile the instrument
   - load the manifest and dylib
   - allocate shared voices for that engine
   - initialize its preset list
   - register it in the engine registry

### Reusing the Same Instrument

If the user loads the same `.lisp` file again:

- do not allocate a new synth backend instance per track
- reuse the same engine
- create a new track bound to that engine
- initialize that track's sound from the engine default or selected preset

### Track Ownership

Each track owns:

- the selected engine
- the current editable sound defaults
- the loaded preset identity, if any
- pattern-local p-locks

Tracks do not own voices directly.

### Preset Semantics

Preset loading is copy-on-load.

That means:

- loading a preset copies its values into the track sound
- later edits modify only the track sound
- presets do not live-update every track using them

Supported user actions:

- `Load Preset`
- `Revert to Loaded Preset`
- `Save as New Preset`
- `Overwrite Loaded Preset`
- `Rename Preset`
- `Delete Preset`

### Pattern Behavior

Track sound must be pattern-local.

Pattern switching restores, per track:

- step triggers
- step params
- synth step p-locks
- track synth default params
- base note offset
- loaded preset id
- dirty state if tracked

This keeps synth edits in one pattern from leaking into others.

## Persistence Model

### Instrument Source File

Existing:

- `instruments/<engine-name>.lisp`

This remains the synth source.

### Preset List File

Each instrument must have a preset list file stored next to the instrument source.

Convention:

- `instruments/<engine-name>.presets`

This is the canonical preset list file for that engine.

This matches the user note: the preset list file can use the instrument name with a different extension.

### Preset File Format

Use JSON content even if the extension is `.presets`.

Example path:

- `instruments/mini-prophet.presets`

Suggested file shape:

```json
{
  "version": 1,
  "engine_name": "mini-prophet",
  "source_file": "instruments/mini-prophet.lisp",
  "source_hash": "sha256:...",
  "presets": [
    {
      "id": "warm-brass",
      "name": "Warm Brass",
      "base_note_offset": 0.0,
      "params": {
        "amp_attack_ms": 8.0,
        "amp_decay_ms": 180.0,
        "cutoff": 420.0
      }
    }
  ]
}
```

Rules:

- store param values by parameter name, not cell id
- unknown params are ignored on load
- missing params fall back to engine defaults
- if the engine schema changes, presets survive best-effort by name matching

## Runtime Architecture

### Engine Registry

Add a central registry on the UI/main side:

```text
EngineRegistry
  engines: Vec<EngineInstance>
  by_source_hash: HashMap<String, EngineId>
  by_source_path: HashMap<PathBuf, EngineId>
```

### Engine Instance

Suggested structure:

```text
EngineInstance
  id: EngineId
  name: String
  source_path: PathBuf
  source_hash: String
  manifest: DGenManifest
  lib: LoadedDGenLib
  voice_count: usize
  voices: Vec<EngineVoice>
  presets: PresetBank
  param_name_to_index: HashMap<String, usize>
```

### Engine Voice

Suggested fields:

```text
EngineVoice
  synth_node_id: i32
  gatepitch_node_id: i32
  logical_id: u64
  active: bool
  age: u64
  assigned_track: Option<usize>
  note: f32
  current_fingerprint: Option<u64>
```

### Track Binding

Replace the current custom-track model with:

```text
TrackSynthBinding
  engine_id: EngineId
  loaded_preset_id: Option<PresetId>
  param_defaults: SlotParamDefaults-like storage
  base_note_offset: f32
  dirty_from_loaded_preset: bool
```

This can likely replace or wrap the current `instrument_slots[track]` plus `instrument_base_note_offsets[track]`.

## Voice Allocation Algorithm

When a note is triggered on track `T`:

1. Resolve `track_binding`.
2. Compute `fingerprint = hash(engine_id + base_note_offset + param_defaults)`.
3. Search the engine voice pool in this order:
   - inactive voice with same fingerprint
   - inactive voice with no fingerprint or different fingerprint
   - active voice with same fingerprint and same track
   - oldest inactive voice
   - oldest active voice
4. If selected voice fingerprint differs from requested fingerprint:
   - push all track sound defaults to that voice's synth node
   - cache the new fingerprint on that voice
5. Push note-level values:
   - pitch
   - velocity
   - gate
   - trigger
6. Mark voice active and update age.

### Notes

- Full param pushes happen only when fingerprint changes.
- If two tracks share an identical sound, voices can be reused without resending the entire parameter vector every note.
- This is the key optimization and the key mental model.

## Audio Thread Changes

### Current Problem

Today the runtime assumes:

- track owns voices
- synth params are broadcast to all synth nodes for that track

That model must change for shared engines.

### New Trigger Flow

For custom engine tracks:

1. Audio thread receives track trigger.
2. Resolve engine id from the track.
3. Ask that engine's voice allocator for a voice using the track sound fingerprint.
4. If the voice needs reconfiguration, push the track sound defaults only to that voice.
5. Push note trigger data to that voice's gatepitch node.
6. Schedule gate-off against that specific engine voice.

### Gate-Off Tracking

Gate-off state must move from "per track voice lid" assumptions to "engine voice logical id" handling.

This is already close to how custom instruments behave, but the ownership source changes.

## UI Specification

### Sidebar Modes

For sampler tracks:

- keep the current sample browser / audition flow

For synth tracks:

- sidebar becomes a preset browser

Suggested sidebar sections:

- engine name
- loaded preset name
- dirty indicator
- preset list
- actions footer

### Sidebar Actions

Required actions:

- `Load`
- `Revert`
- `Save New`
- `Overwrite`
- `Rename`
- `Delete`

Possible keymap:

- `Enter` = load preset
- `Ctrl+S` = save new
- `Ctrl+O` = overwrite loaded preset
- `r` = rename
- `Backspace/Delete` = delete preset

This is provisional; exact bindings can be chosen later.

### Synth Panel

The current Synth tab remains the editor for the track-owned sound.

Additional UI metadata should be shown somewhere in the panel:

- `Engine: mini-prophet`
- `Preset: Warm Brass`
- `*` dirty marker if the track sound differs from the loaded preset

### Pattern Switching UX

On pattern switch:

- restore the track sound snapshot
- immediately push restored synth defaults into live engine voices for future note allocation
- no forced retrigger of currently playing notes in V1

That means:

- new notes use restored sound immediately
- already sounding notes may continue with the sound they were configured with

This is the safest V1 behavior.

## Data Model Changes

### Sequencer State

Add or replace these areas:

- `engine_bindings_per_track`
- `pattern-local track sound snapshots`
- eventually remove track-owned synth node arrays for custom engines

### Pattern Snapshot

Pattern snapshots must include:

- `TrackSynthBindingSnapshot` per track
  - `engine_id`
  - `loaded_preset_id`
  - `param_defaults`
  - `base_note_offset`
  - optional `dirty_from_loaded_preset`
- synth step p-locks

### Graph State

Graph state must move from:

- `track_synth_node_ids`
- `track_gatepitch_node_ids`

to something like:

- `engine_voice_nodes[engine_id]`

Tracks still need enough metadata to route note events to the correct engine.

## Preset Bank API

### Load

```text
load_preset_list(engine_source_path) -> PresetBank
```

Behavior:

- look for `instruments/<name>.presets`
- if missing, create an empty bank in memory
- do not fail engine load because no preset file exists

### Save

```text
save_preset_list(engine_source_path, bank) -> Result<(), String>
```

Behavior:

- write back to `instruments/<name>.presets`
- atomic write preferred

### Operations

```text
load_preset_into_track(track, preset_id)
save_track_as_new_preset(track, name)
overwrite_loaded_preset_from_track(track)
rename_preset(engine_id, preset_id, new_name)
delete_preset(engine_id, preset_id)
```

## Schema Evolution Rules

Presets must load by parameter name.

When loading a preset:

- if preset param exists in engine schema, apply it
- if preset param is missing in schema, ignore it
- if engine param is missing from preset, use manifest default

If source hash changed:

- engine may still reuse the preset file
- preset compatibility is best-effort by parameter name

Do not key presets by cell id. Cell ids are compiler/runtime implementation details.

## Implementation Plan

### Phase 1: Preset Persistence on Current Track-Owned Model

Goal:

- add presets without changing shared-voice architecture yet

Tasks:

1. Add `.presets` file loader/saver per instrument.
2. Add `TrackSound` abstraction on top of current `instrument_slots`.
3. Add preset browser UI for synth tracks.
4. Add save/load/revert/overwrite flows.
5. Keep current per-track custom voice allocation unchanged.

This phase gives immediate value with lower risk.

### Phase 2: Engine Registry

Goal:

- deduplicate loading same instrument source multiple times

Tasks:

1. Add `EngineRegistry`.
2. Reuse already-loaded engine when same source is selected again.
3. Keep track-owned runtime voices temporarily if needed.

This phase separates engine identity from track identity.

### Phase 3: Shared Engine Voice Pool

Goal:

- move custom voices from per-track ownership to per-engine ownership

Tasks:

1. Allocate voices per engine.
2. Introduce engine voice allocator with fingerprint matching.
3. Route note triggers through engine voice selection.
4. Remove track-owned custom synth/gatepitch node assumptions.

This is the largest runtime refactor.

### Phase 4: Cleanup + UX

Tasks:

1. Show engine/preset metadata in UI.
2. Add dirty-state indicators.
3. Add overwrite confirmation flow.
4. Add preset rename/delete UX.
5. Add import/export if desired.

## Recommended V1 Boundary

Recommended first ship:

- `.presets` file per instrument
- track-owned editable sound state
- synth preset browser in sidebar
- pattern-local track sound snapshots
- copy-on-load preset model
- no shared engine voice pool yet

Reason:

- most of the user value arrives here
- much lower runtime risk
- shared engine voices can come later without invalidating the preset UX

## Open Questions

These should be resolved before Phase 3:

1. How many voices should each engine allocate by default?
2. Should engine voice count be user-configurable per engine?
3. Should preset load update currently sounding notes or only future notes?
4. Should tracks remember `loaded_preset_id` separately per pattern?
5. Should factory presets live inside `.presets`, or should there be separate read-only and user banks?
6. Should fingerprint include only sound defaults, or also engine version/hash?

## Recommended Answers

Recommended defaults:

1. Allocate `12` voices per engine initially.
2. Make voice count fixed in V1.
3. Only future notes use new preset/default values in V1.
4. Yes, `loaded_preset_id` should be pattern-local if track sound is pattern-local.
5. Keep one `.presets` file in V1; add factory/user split later if needed.
6. Include `engine_id` and sound values in the fingerprint; engine version/hash is already implied by `engine_id`.
