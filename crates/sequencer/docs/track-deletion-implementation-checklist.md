# Track Deletion Implementation Checklist

This checklist breaks track deletion into implementation phases with exact files, target functions, order of operations, and required verification.

## Goal

Delete one track safely while the app is running, compact all track-indexed state, remap dependent references, refresh the Lisp/reactive UI, and preserve correct behavior across pattern switching and project save/load.

## Behavior To Preserve

- [ ] Deleting a track removes it from the live graph, current pattern, all in-memory patterns, and reactive UI state.
- [ ] Tracks after the deleted index shift down by one.
- [ ] The selected track becomes `min(deleted_idx, new_last_idx)`.
- [ ] Deleting the last remaining track is blocked.
- [ ] Pattern switching after deletion does not resurrect removed tracks.
- [ ] Saving and reloading a project after deletion preserves the compacted track layout.

## Phase 1: Lock Deletion Semantics

### Files

- [ ] Update comments and intent near [src/ttui/graph.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ttui/graph.rs)
- [ ] Update comments and intent near [src/ui/main.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ui/main.rs)

### Checklist

- [ ] Decide that deleting the last remaining track is invalid and should show a status message.
- [ ] Decide that delete compacts all track indices immediately.
- [ ] Decide that delete applies to the live arrangement and all pattern snapshots currently loaded in memory.
- [ ] Decide that sidechain references to the deleted track reset to `off`.
- [ ] Decide that sidechain references above the deleted track decrement by 1.
- [ ] Add a short comment above the main delete entrypoint documenting these rules.

### Verification

- [ ] Read through the implementation entrypoint and confirm the rules are documented in one place.

## Phase 2: Add Snapshot-Level Track Removal

### Files

- [ ] Edit [src/sequencer/state.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/sequencer/state.rs)

### Functions To Add Or Update

- [ ] Add `PatternSnapshot::remove_track(track_idx: usize)`

### Order Of Ops

- [ ] In `PatternSnapshot::remove_track`, remove `track_idx` from `track_bits`.
- [ ] Remove `track_idx` from `step_data`.
- [ ] Remove `track_idx` from `track_params`.
- [ ] Remove `track_idx` from `effect_slots`.
- [ ] Remove `track_idx` from `instrument_slots`.
- [ ] Remove `track_idx` from `instrument_base_note_offsets`.
- [ ] Remove `track_idx` from `track_sound_states`.
- [ ] Remove `track_idx` from `sample_ids`.
- [ ] Remove `track_idx` from `chord_snapshots`.
- [ ] Remove `track_idx` from `timebase_plock_snapshots`.
- [ ] Remove `track_idx` from `swing_plock_snapshots`.
- [ ] Remove `track_idx` from `swing_resolution_plock_snapshots`.
- [ ] Remove `track_idx` from `instrument_types`.
- [ ] Guard against out-of-range indices without panicking.

### Unit Tests

- [ ] Add a test that `PatternSnapshot::remove_track` compacts every track-indexed vector consistently.
- [ ] Add a test that removing the first track shifts remaining data correctly.
- [ ] Add a test that removing a middle track preserves before/after neighbors.
- [ ] Add a test that out-of-range removal is a no-op or returns cleanly, depending on the final API.

### Manual Verification

- [ ] No app run needed yet if this phase is purely state-level.

## Phase 3: Add Live Sequencer-State Track Removal Helpers

### Files

- [ ] Edit [src/sequencer/state.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/sequencer/state.rs)

### Functions To Add Or Update

- [ ] Add `SequencerState::remove_track(track_idx: usize)` or equivalent helper on the appropriate live-state type
- [ ] Add a helper to compact live pattern state for the active pattern
- [ ] Add a helper to compact runtime per-track arrays

### Order Of Ops

- [ ] Remove `track_idx` from live `pattern.patterns`.
- [ ] Remove `track_idx` from live `pattern.step_data`.
- [ ] Remove `track_idx` from live `pattern.track_params`.
- [ ] Remove `track_idx` from live `pattern.effect_chains`.
- [ ] Remove `track_idx` from live `pattern.instrument_slots`.
- [ ] Remove `track_idx` from live `pattern.instrument_base_note_offsets`.
- [ ] Remove `track_idx` from live `pattern.track_sound_state`.
- [ ] Remove `track_idx` from live `pattern.chord_data`.
- [ ] Remove `track_idx` from live `pattern.timebase_plocks`.
- [ ] Remove `track_idx` from live `pattern.swing_plocks`.
- [ ] Remove `track_idx` from live `pattern.swing_resolution_plocks`.
- [ ] Shift runtime arrays left for all tracks above `track_idx`.
- [ ] Clear the now-unused last runtime slot to zero/default values.
- [ ] Decrement `transport.num_tracks`.
- [ ] Publish a scheduler snapshot after compaction is complete.

### Runtime Arrays To Audit

- [ ] `runtime.voice_lids`
- [ ] `runtime.voice_counts`
- [ ] `runtime.sampler_lids`
- [ ] `runtime.pan_lids`
- [ ] `runtime.delay_lids`
- [ ] `runtime.send_lids`
- [ ] `runtime.instrument_type_flags`
- [ ] `runtime.track_engine_ids`
- [ ] Any pending per-track reset flags
- [ ] Any additional per-track arrays introduced since this checklist was written

### Unit Tests

- [ ] Add a test that live state removal decrements active track count.
- [ ] Add a test that live state removal shifts track params and step data correctly.
- [ ] Add a test that the runtime slot above the new active range is cleared.

### Manual Verification

- [ ] No app run required yet if the helper is still unhooked.

## Phase 4: Add Sidechain And Cross-Track Reference Remapping

### Files

- [ ] Edit [src/tui/effects.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/tui/effects.rs)
- [ ] Edit [src/sequencer/state.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/sequencer/state.rs) if remap helpers belong closer to snapshots/state
- [ ] Audit [src/accumulator.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/accumulator.rs) for raw track references

### Functions To Add Or Update

- [ ] Add a helper to remap effect sidechain defaults/plocks after track deletion
- [ ] Add a helper to apply the remap to the live pattern
- [ ] Add a helper to apply the remap to every `PatternSnapshot`

### Order Of Ops

- [ ] Identify sidechain params via `HostControl::FxSidechain`.
- [ ] For each track and effect slot, inspect default sidechain selection.
- [ ] If selection points at deleted track, rewrite to `off`.
- [ ] If selection points above deleted track, decrement by 1.
- [ ] Apply the same remap to per-step plocks for sidechain params if present.
- [ ] Audit accumulator/send-to-track references and apply equivalent remapping if they are persisted by track index.

### Unit Tests

- [ ] Add a test that sidechain defaults referencing the deleted track become `off`.
- [ ] Add a test that sidechain defaults above the deleted track decrement by 1.
- [ ] Add a test that sidechain plocks are remapped with the same rules.

### Manual Verification

- [ ] Create two tracks where one sidechains from the other, delete the source track, and confirm the selection resets cleanly.
- [ ] Create three tracks where an effect sidechains from track 3, delete track 2, and confirm the selection now points to the shifted track 2.

## Phase 5: Add Single-Track Graph Teardown

### Files

- [ ] Edit [src/ttui/graph.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ttui/graph.rs)

### Functions To Add Or Update

- [ ] Add `GraphController::delete_track(track_idx: usize) -> Result<usize, String>`
- [ ] Add helper to remove custom FX nodes for one track
- [ ] Add helper to tear down one track shell
- [ ] Add helper to determine whether a custom engine runtime is still referenced by any remaining track

### Order Of Ops

- [ ] Validate `track_idx`.
- [ ] Reject delete if `self.app.tracks.len() <= 1`.
- [ ] Begin `GraphEditBatchGuard`.
- [ ] Remove custom FX nodes on the track, disconnecting predecessor and successor correctly.
- [ ] Remove any sidechain wiring into those FX before deleting the nodes.
- [ ] Delete sampler nodes for sampler tracks.
- [ ] Delete send, delay, filter, pan, voice sum nodes for the track.
- [ ] Remove the pan node from the watchlist before deleting it.
- [ ] Do not delete shared custom-engine runtime nodes unless no remaining track references that engine.
- [ ] After graph teardown, compact app-level vectors and sequencer state.
- [ ] Recompute the new selected track index.
- [ ] Return the new selected track index.

### Shared Runtime Audit

- [ ] Confirm whether `graph.engine_node_ids` are shared across tracks.
- [ ] Confirm whether deleting a custom track should only remove its binding, not the engine runtime.
- [ ] Add cleanup of unused engine runtime only if the last bound track for that engine is removed.

### Unit Tests

- [ ] Add a focused test for helper logic that decides whether an engine runtime is still referenced.

### Manual Verification

- [ ] Delete a sampler track and confirm audio continues on remaining tracks.
- [ ] Delete a custom instrument track and confirm other tracks using the same engine still work.
- [ ] Delete a track with custom FX and confirm the remaining audio chain is intact.

## Phase 6: Compact App-Level Indexed Collections

### Files

- [ ] Edit [src/ttui/graph.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ttui/graph.rs)
- [ ] Edit [src/tui/mod.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/tui/mod.rs) if helper methods are needed

### Collections To Compact

- [ ] `self.app.tracks`
- [ ] `self.app.sampler_paths`
- [ ] `self.app.graph.track_node_ids`
- [ ] `self.app.graph.track_buffer_ids`
- [ ] `self.app.graph.track_voice_lids`
- [ ] `self.app.graph.track_instrument_types`
- [ ] `self.app.graph.track_engine_ids`
- [ ] `self.app.graph.track_synth_node_ids`
- [ ] `self.app.graph.track_gatepitch_node_ids`
- [ ] `self.app.graph.effect_descriptors`
- [ ] `self.app.graph.instrument_descriptors`
- [ ] `self.app.graph.record_armed`

### Order Of Ops

- [ ] Remove `track_idx` from each app-level per-track vector in one contiguous block.
- [ ] Clamp `ui.cursor_track` to the returned selected track index.
- [ ] Clamp `ui.cursor_step` if needed for the newly selected track.
- [ ] Clear or clamp any UI state that assumed the deleted track still exists.

### Unit Tests

- [ ] Add a small state-level test if there is a helper factored out for vector compaction.

### Manual Verification

- [ ] Confirm track names, arm state, and selected track all shift correctly in the UI after deletion.

## Phase 7: Compact Pattern Bank Snapshots

### Files

- [ ] Edit [src/ttui/graph.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ttui/graph.rs)
- [ ] Edit [src/sequencer/state.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/sequencer/state.rs)

### Functions To Add Or Update

- [ ] Add a helper to iterate `pattern_bank` and call `PatternSnapshot::remove_track(track_idx)`

### Order Of Ops

- [ ] Capture any current state needed before modifying snapshots.
- [ ] Iterate all entries in `state.pattern.pattern_bank`.
- [ ] Remove the track from every snapshot, including the current pattern snapshot representation.
- [ ] Ensure current pattern index remains valid.
- [ ] Ensure future pattern switches restore the compacted layout, not the pre-delete layout.

### Unit Tests

- [ ] Add a test that deleting a track and then restoring another pattern does not reintroduce it.

### Manual Verification

- [ ] Create multiple patterns, delete a track, switch across all patterns, and confirm the track count remains compacted.
- [ ] Clone a pattern before deletion, delete a track, switch between original and clone, and confirm consistency.

## Phase 8: Centralize Post-Topology UI Sync

### Files

- [ ] Edit [src/ui/main.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ui/main.rs)

### Functions To Add Or Update

- [ ] Extract a helper such as `sync_all_track_topology_state(...)`
- [ ] Reuse it from add-track paths if practical

### Reactive Values To Refresh

- [ ] `SEQ.num-tracks`
- [ ] `SEQ.current-track`
- [ ] `SEQ.track-names`
- [ ] `SEQ.steps`
- [ ] step param lists
- [ ] `SEQ.track-volumes`
- [ ] `SEQ.track-peaks`
- [ ] `SEQ.effects`
- [ ] `SEQ.instrument-panel`
- [ ] accumulator names
- [ ] track params
- [ ] `SEQ.step-has-plocks`
- [ ] sidebar/browser state
- [ ] `SEQ.record-armed`

### Order Of Ops

- [ ] Build one helper that takes the selected track index and updates all reactive values.
- [ ] Call it after track deletion.
- [ ] Optionally replace duplicated add-track sync blocks with the same helper.
- [ ] Run reactive cycle and refresh runtime side effects once per topology change.

### Unit Tests

- [ ] Add tests only if this helper can be covered without heavyweight UI setup. Otherwise rely on manual verification.

### Manual Verification

- [ ] Delete a track and confirm the mixer, sequencer grid, FX pane, instrument pane, and sidebar all update in the same frame.

## Phase 9: Add Host Command For Delete

### Files

- [ ] Edit [src/ui/main.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/ui/main.rs)

### Functions To Add Or Update

- [ ] Add host command handler for `"delete-track"`

### Order Of Ops

- [ ] Parse `:track` from payload, defaulting to current track if omitted.
- [ ] Call `app.graph_controller().delete_track(track_idx)`.
- [ ] Store the returned selected track index in `current_track`.
- [ ] Call the centralized reactive sync helper.
- [ ] Emit a status message on success.
- [ ] Emit a clear status message if deletion is blocked or fails.

### Manual Verification

- [ ] Trigger the host command directly and confirm the app remains stable.
- [ ] Confirm status messages are useful for both success and invalid delete attempts.

## Phase 10: Add Mixer Delete UX

### Files

- [ ] Edit [ui/legacy/mixer.lisp](/Users/alecresende/code/learning/anthropic/sequencer/ui/legacy/mixer.lisp)

### Checklist

- [ ] Add an `x` affordance on the selected track row only.
- [ ] Place it to the right of the slider and meter block.
- [ ] Send `host-command "delete-track"` with the selected row index.
- [ ] Hide or disable the `x` when only one track remains.
- [ ] Keep the click target larger than the visible glyph.

### Manual Verification

- [ ] Select a track and confirm only that row shows the delete affordance.
- [ ] Delete the first track from the mixer UI.
- [ ] Delete a middle track from the mixer UI.
- [ ] Delete the last track from the mixer UI.
- [ ] Confirm the affordance is absent or disabled when one track remains.

## Phase 11: Project Save/Load Validation

### Files

- [ ] Audit [src/tui/projects.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/tui/projects.rs)
- [ ] Audit [src/project.rs](/Users/alecresende/code/learning/anthropic/sequencer/src/project.rs)

### Checklist

- [ ] Confirm saved project track counts reflect the compacted layout after deletion.
- [ ] Confirm custom effects remain aligned with shifted tracks after deletion.
- [ ] Confirm deleted tracks do not reappear after save/load.

### Unit Tests

- [ ] Add or extend a project serialization test to cover save/load after track deletion if practical.

### Manual Verification

- [ ] Delete a track, save the project, reload it, and confirm layout/state stay compacted.
- [ ] Repeat with a project containing sampler tracks, custom tracks, and custom FX.

## Phase 12: Full Manual Test Matrix

### Core Cases

- [ ] Delete first track
- [ ] Delete middle track
- [ ] Delete last track
- [ ] Attempt to delete only remaining track
- [ ] Delete current track while transport is stopped
- [ ] Delete current track while transport is playing

### Track Type Cases

- [ ] Delete sampler track
- [ ] Delete custom instrument track
- [ ] Delete track with custom FX
- [ ] Delete track with no custom FX

### Pattern Cases

- [ ] Delete after cloning a pattern
- [ ] Delete with multiple patterns present
- [ ] Delete, switch patterns, then switch back

### Cross-Reference Cases

- [ ] Delete track that is used as an FX sidechain source
- [ ] Delete track below another sidechain source and confirm index shift
- [ ] Delete track with record arm enabled

### Persistence Cases

- [ ] Delete, save project, reload project
- [ ] Delete, save project with custom track engines, reload project

## Suggested Execution Order

- [ ] Phase 1
- [ ] Phase 2
- [ ] Phase 3
- [ ] Phase 4
- [ ] Phase 5
- [ ] Phase 6
- [ ] Phase 7
- [ ] Phase 8
- [ ] Phase 9
- [ ] Phase 10
- [ ] Phase 11
- [ ] Phase 12

## Definition Of Done

- [ ] Track deletion works from the mixer UI.
- [ ] The app does not crash or desync after deleting a track.
- [ ] Pattern switching after deletion is stable.
- [ ] Save/load after deletion is stable.
- [ ] All listed unit tests added for implemented helpers pass.
- [ ] Manual test matrix completed successfully.
