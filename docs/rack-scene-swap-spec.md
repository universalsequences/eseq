# Rack Scene-Change Swap Spec

**Status:** Layers 1+2 ready to implement. Layer 3 is design-level, deferred.
**Branch context:** written against `codex/fx-chain-host-r1` (line numbers cited below are from that branch and will drift — treat them as anchors, not gospel; the function names are the stable references).

## Motivation

Two goals, one immediate and one long-term:

1. **Immediate bug:** a quantized (1/4) scene change cuts off the kick that lands on the
   boundary. Root cause (verified): every scene change unconditionally tears down and
   rebuilds every instrument rack's audio nodes, even when the old and new scenes have
   identical rack configurations. The boundary-straddling voice starts on the old nodes
   and then those nodes are deleted out from under it.
2. **Long-term vision ("the system"):** an entire live set in one project — a fixed set
   of tracks (~16), each holding an instrument rack, and hundreds of scenes that
   hot-swap the rack contents as the set progresses (Autechre-rig style: channels with
   sequencer/instrument/FX slots swapped per section). This only works if scene changes
   that *don't* change a rack are free, scene changes that *do* change a rack never
   audibly glitch, and (eventually) the expensive swap work happens ahead of the
   quantized boundary.

## Verified current behavior (the bug path)

All in `crates/sequencer/src/`:

1. `tui/mod.rs` — `App::apply_pattern_launch` (~line 913) calls
   `state.launch_scene(...)`, which restores the target scene's complete per-track
   state including `pattern.rack_tracks` (the `RackTrackSnapshot` per rack track,
   restored in `apply_track_pattern_data` via `self.rack_tracks[track] = data.rack_track`,
   `sequencer/state.rs:2567`). It then calls
   `self.graph_controller().apply_sample_ids(&sample_ids)` (`tui/mod.rs:976`).
2. `tui/graph.rs` — `apply_sample_ids` (~3502) unconditionally ends with
   `self.sync_live_rack_tracks_from_pattern_state()` (~3523).
3. `sync_live_rack_tracks_from_pattern_state` (~3531) loops over every track whose
   `track_instrument_types[track] == InstrumentType::Rack` and calls
   `rebuild_rack_slot_graph(track_idx, &mut rack)` (~3544) **with no comparison**
   against the live topology. It then writes the rack back into `pattern.rack_tracks`,
   calls `state.sync_rack_slot_instrument_bindings_for_current_pattern(track, &bindings)`,
   and if anything was rebuilt: `schedule_mod_resync()`, `request_all_accumulator_resets()`,
   `publish_scheduler_snapshot()`, and a `topology_epoch` bump (~3568).
4. `rebuild_rack_slot_graph` (~5080) does real teardown inside a
   `GraphEditBatchGuard`: `delete_engine_route_for_track` for each engine,
   `delete_rack_slot_nodes` for each slot (deletes sampler voices, modulators,
   gate-pitch nodes, slot pan, slot L/R sums — `~4196`), and
   `clear_rack_sampler_runtime_pools_for_track` (~5007). Then it recreates everything:
   `create_rack_slot_mixer`, `build_sampler_voices` / `connect_engine_to_track`,
   `publish_sampler_voice_runtime`, `connect_fx_chain_host` per slot,
   `publish_rack_slot_panner_runtime`. Finally it reaps engine runtimes that are no
   longer referenced (`engine_is_still_referenced` / `delete_engine_runtime`).
5. Quantized launches fire **after** the boundary has been rendered:
   `quantized_launch.rs` `PendingQuantizedLaunches::process` marks a launch due when
   `deadline_beats <= rendered_beats + BOUNDARY_EPSILON_BEATS` (~139), and the UI loop
   drains it via `app.drain_due_pattern_launches()` (`ui/main.rs` ~6780). So the
   on-the-beat kick has already been triggered on the old sampler nodes when the
   rebuild deletes them.
6. The `topology_epoch` bump makes the scheduler clear its queue and rebuild its
   lookahead horizon (`scheduler.rs` ~7598).

**Important existing facts the implementation relies on:**

- FX-chain nodes are **not** owned by the rebuild. `delete_rack_slot_nodes` deletes
  samplers/modulators/gatepitch/pan/sums only; the per-slot FX nodes referenced by
  `RackSlotSnapshot::effect_slots[].node_id` survive and are merely re-wired by
  `connect_fx_chain_host`.
- Live in-place setters already exist for every slot mixer parameter:
  `set_rack_slot_gain` / `set_rack_slot_pan` / `set_rack_slot_mute` /
  `set_rack_slot_solo` / `set_rack_slot_max_polyphony` / `set_rack_slot_base_note_offset`
  (`tui/synth.rs:1292+`), built on `push_rack_slot_panner_param` (~1239) and
  `push_rack_slot_solo_mutes` (~1260). Max-polyphony explicitly does **not** require a
  rebuild — `VoicePool::allocate_voice_retriggering_same_note_with_limit` self-clamps
  to the built voice count (see the comment at `synth.rs:1349`).
- Track-level samplers already swap samples in place on scene change via
  `send_sample_to_all_voices` (`tui/graph.rs:3974`) — plain `params_push_wrapper` of
  `PARAM_BUFFER_ID` + `PARAM_SOURCE_SAMPLE_RATE` by voice logical id. Rack slots can do
  exactly the same using `RackSlotNodeIds::sampler_voice_lids`.
- Sampler voice pools are keyed by position:
  `rack_slot_pool_index(track_idx, slot_idx) = MAX_TRACKS + track_idx * MAX_RACK_SLOTS + slot_idx`
  (`sequencer/data.rs:28`). The pool runtime (`publish_sampler_voice_runtime` /
  `clear_sampler_runtime_pool`) is allocation bookkeeping; the voice *nodes* are
  independent graph nodes.
- `rebuild_rack_slot_graph` has exactly five call sites: four user-edit paths
  (`tui/graph.rs` ~2658, ~2963, ~3170, ~3340 — convert-to-rack, slot add/remove,
  instrument swap, etc.) and the scene-sync path (~3544). **Only the scene-sync path
  gets the diff gate.** User edits always rebuild; that keeps the signature cache
  trivially fresh (see below).

---

## Architecture: three layers

- **Layer 1 — semantic topology diff:** skip the rebuild entirely when the incoming
  scene's rack topology matches the live one; apply parameters in place. Makes the
  common case (hundreds of scenes, same instruments, different patterns/params) free.
- **Layer 2 — deferred teardown:** when a rack genuinely changes, don't delete the old
  nodes at the boundary; let them ring out and reap them later. Makes real swaps
  inaudible.
- **Layer 3 — prepare-ahead (future):** use the quantized-launch queue's lead time to
  pre-build the new rack graph before the boundary, so the commit is just reconnection.

**Recommendation: implement Layers 1+2 now, as one unit of work.** Layer 1 fixes the
reported bug for the identical-config case (the case the user actually hit:
rack.sampler → rack.sampler). Layer 2 fixes it for genuine swaps. Layer 3 is an
optimization to add only if a synchronous boundary rebuild measurably stalls the UI
loop (`PatternSwitchProfile` in `sequencer/state.rs` already exists for timing this).

---

## Layer 1 — Semantic topology diff

### 1.1 The signature type

New file or a section in `tui/graph.rs` (near the rack functions):

```rust
/// Structural identity of a rack's live audio graph. Two racks with equal
/// signatures can share the same set of graph nodes; everything not captured
/// here is a parameter that can be applied to existing nodes in place.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RackTopologySignature {
    pub slots: Vec<RackSlotTopologySignature>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RackSlotTopologySignature {
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    /// Source identity for Custom/Modulator slots; None for Sampler.
    pub engine_id: Option<usize>,
    /// FX chain wiring identity: (node_id, modulator_node_id, in_ch, out_ch)
    /// for each occupied effect slot, in order.
    pub fx_chain: Vec<(u32, u32, u32, u32)>,
}
```

Computation is a pure function of the snapshot — this is what makes it unit-testable:

```rust
fn rack_topology_signature(rack: &RackTrackSnapshot) -> RackTopologySignature {
    RackTopologySignature {
        slots: rack.slots.iter().map(|slot| RackSlotTopologySignature {
            instrument_type: slot.instrument_type,
            instrument_run_mode: slot.instrument_run_mode,
            engine_id: match slot.instrument_type {
                InstrumentType::Custom | InstrumentType::Modulator => {
                    slot.track_sound_state.engine_id
                }
                _ => None,
            },
            fx_chain: slot.effect_slots.iter().zip(&slot.effect_descriptors)
                .filter(|(effect, _)| effect.node_id != 0)
                .map(|(effect, desc)| (
                    effect.node_id,
                    effect.modulator_node_id,
                    desc.input_channels,
                    desc.output_channels,
                ))
                .collect(),
        }).collect(),
    }
}
```

Adjust field access to the real `EffectSlotSnapshot` field names/types (check how
`rebuild_rack_slot_graph` reads them when building `FxChainSlotView`, ~5270 — it uses
`effect.node_id as i32`, `effect.modulator_node_id as i32`, `descriptor.input_channels`,
`descriptor.output_channels`). The "occupied" filter should match however
`connect_fx_chain_host` distinguishes empty slots — if it doesn't filter, don't filter
here either; the point is that the signature is equal iff the wiring
`connect_fx_chain_host` would produce is equal.

**Deliberately excluded from the signature (these are parameter-level, not topology):**

| Field | Why excluded | In-place mechanism |
|---|---|---|
| `sample_id` (buffer) | sample swap must not rebuild | param push to `sampler_voice_lids` (like `send_sample_to_all_voices`) |
| `gain`, `pan`, `mute`, `solo` | slot mixer params | `push_rack_slot_panner_param` / `push_rack_slot_solo_mutes` |
| `max_polyphony` | pool self-clamps (see `synth.rs:1349` comment) | nothing needed |
| `instrument_base_note_offset`, `pad_note`, `choke_group`, `param_plocks` | scheduler-side, read from snapshot at trigger time | nothing needed |
| `rack.routing`, `rack.macros` | scheduler/state-side, no graph nodes | nothing needed |
| instrument param values in `instrument_slot` | applied via param routes / scheduler snapshot | nothing needed beyond binding re-sync (below) |

### 1.2 Caching the live signature

The old topology is **not** recoverable at diff time — `launch_scene` has already
overwritten `pattern.rack_tracks` with the new scene's snapshots before
`sync_live_rack_tracks_from_pattern_state` runs. So the live graph's signature must be
cached at build time.

Add the cache to the per-track graph node struct — the one holding
`rack_slots: Vec<RackSlotNodeIds>` (find it via that field; it's the element type of
`app.graph.track_node_ids`). Add:

```rust
pub rack_signature: Option<RackTopologySignature>,
```

initialized to `None` wherever the struct is constructed (`create_track_shell` callers,
`Default`/literal constructions — the compiler will enumerate them). Because this rides
inside `track_node_ids`, it automatically follows track add/remove/reorder/compaction —
no separate lifecycle to manage.

At the **end** of `rebuild_rack_slot_graph` (after `track_node_ids[track_idx].rack_slots`
is assigned, before returning), set:

```rust
self.app.graph.track_node_ids[track_idx].rack_signature =
    Some(rack_topology_signature(rack));
```

Note: compute it from `rack` *after* the loop has run
`slot.instrument_slot.sync_to_descriptor_with_modulator(...)` mutations — FX node ids
in `effect_slots` are unaffected by those, but compute-at-the-end is the simple rule.
Since all five `rebuild_rack_slot_graph` call sites go through this one function, the
cache can never go stale relative to the live graph. `None` (never built / track just
converted) always means "rebuild".

### 1.3 The gate in `sync_live_rack_tracks_from_pattern_state`

Replace the unconditional rebuild (~3544) with:

```rust
let incoming_sig = rack_topology_signature(&rack);
let live_sig = self.app.graph.track_node_ids[track_idx].rack_signature.clone();

let bindings = if live_sig.as_ref() == Some(&incoming_sig)
    && self.validate_rack_slot_graph_rebuild(track_idx, &rack).is_ok()
{
    self.apply_rack_scene_state_in_place(track_idx, &mut rack)?
} else {
    rebuilt_any = true; // only the rebuild path sets this
    self.rebuild_rack_slot_graph(track_idx, &mut rack)?
};
```

Keep the existing post-processing identical for both paths: write `rack` back into
`pattern.rack_tracks[track_idx]`, then
`sync_rack_slot_instrument_bindings_for_current_pattern(track_idx, &bindings)` (this
already updates both the live rack and the current scene's pooled pattern data —
`sequencer/state.rs:4570`).

The `if rebuilt_any { ... }` tail block (`schedule_mod_resync`,
`request_all_accumulator_resets`, `publish_scheduler_snapshot`, `topology_epoch` bump)
**only fires when a real rebuild happened**. That's the payoff: an identical-topology
scene change no longer bumps `topology_epoch`, so the scheduler keeps its lookahead
horizon and the boundary voice's nodes are never touched. Add a separate, cheaper tail
for the in-place path: if any rack took the in-place path, call
`self.app.state.publish_scheduler_snapshot()` once (bindings/param defaults may have
changed values even though node ids didn't; publishing is cheap and idempotent) — but
**no** epoch bump, no accumulator reset, no mod resync.

The `validate_...is_ok()` guard covers restored snapshots that are structurally equal
but semantically broken (e.g., a sampler slot whose `sample_id` is `None`, or an engine
id no longer in the registry after a project edit) — those fall through to the rebuild
path, which produces the existing user-visible error.

### 1.4 `apply_rack_scene_state_in_place`

New method on the graph controller (same impl block as `rebuild_rack_slot_graph`).
Signature mirrors the rebuild so the call site stays symmetric:

```rust
fn apply_rack_scene_state_in_place(
    &mut self,
    track_idx: usize,
    rack: &mut RackTrackSnapshot,
) -> Result<Vec<(EffectDescriptor, u32, u32)>, String>
```

Steps, per slot (`for (slot_idx, slot) in rack.slots.iter_mut().enumerate()`), reading
live nodes from `self.app.graph.track_node_ids[track_idx].rack_slots[slot_idx]`:

1. **Rebuild the bindings from the LIVE nodes** — never trust node ids inside the
   restored snapshot; scenes captured under an older graph incarnation carry stale ids.
   Mirror exactly what `rebuild_rack_slot_graph` pushes:
   - Sampler slot: `descriptor = EffectDescriptor::builtin_sampler()`;
     `node_id = first_graph_node_identity(&nodes.sampler_ids)`;
     `modulator_node_id = first_graph_node_identity(&nodes.sampler_modulator_ids)`.
   - Custom/Modulator slot: `engine_id = nodes.engine_id` (equal to the snapshot's by
     signature match); look up `engine_registry.get(engine_id)` for the name/manifest,
     `descriptor = lisp_host::instrument_descriptor_from_manifest(&name, &manifest)`;
     node ids from `self.app.graph.engine_node_ids[engine_id]`'s `synth_ids` /
     `modulator_ids` via `first_graph_node_identity`. If the engine runtime is missing
     (`engine_node_ids[engine_id]` is `None`), bail with `Err` — the caller's `?`
     surfaces it and the next launch attempt will take the rebuild path (signature
     can't have matched a live graph in that state; treat as defensive).
   - Then `slot.instrument_slot.sync_to_descriptor_with_modulator(&descriptor, node_id, modulator_node_id)`
     — same call the rebuild makes — and push `(descriptor, node_id, modulator_node_id)`
     onto the returned `bindings` vec.
2. **Sample swap in place** (Sampler slots only): unconditionally push the snapshot's
   sample to the live voices — pushing an unchanged value is harmless, and we don't
   know the old buffer:
   ```rust
   if let Some((buffer_id, _name, sample_rate)) = slot.sample_id.clone() {
       for &lid in &nodes.sampler_voice_lids {
           // params_push_wrapper PARAM_BUFFER_ID = buffer_id as f32
           // params_push_wrapper PARAM_SOURCE_SAMPLE_RATE = sample_rate.max(1) as f32
       }
   }
   ```
   (Copy the exact unsafe push pattern from `send_sample_to_all_voices`,
   `tui/graph.rs:3974`.) Note: this retargets ringing voices' buffers mid-tail, which
   is exactly the parity behavior track-level samplers already have on scene change.
3. **Slot mixer params**: push the snapshot's mixer state to the live panner node via
   the existing helper (`push_rack_slot_panner_param`, `tui/synth.rs:1239` — it
   resolves `slot_pan_id` internally):
   - `STEREO_PANNER_PARAM_VOLUME` ← `slot.gain`
   - `STEREO_PANNER_PARAM_PAN` ← `slot.pan`
   - `STEREO_PANNER_PARAM_MUTE` ← handled by step 4, don't push raw `slot.mute` here
     if `push_rack_slot_solo_mutes` already folds mute+solo together — **read
     `push_rack_slot_solo_mutes` (~synth.rs:1260) first** and replicate whichever
     mute/solo composition it uses so solo semantics (`has_solo && !slot.solo`) match
     the rebuild path's `create_rack_slot_mixer` args.
   Note `push_rack_slot_panner_param` lives on a different controller type
   (`tui/synth.rs` impl) than the graph controller — if it isn't reachable from the
   graph controller's `self`, replicate its ~10-line body (it's a
   `params_push_wrapper` on the slot pan node) as a private helper rather than
   restructuring controllers.
4. **Solo/mute matrix**: after the per-slot loop, apply the cross-slot solo state once
   (equivalent of `push_rack_slot_solo_mutes(track_idx)`), then
   `self.publish_rack_slot_panner_runtime(track_idx)` (~4992) so UI meters/pan runtime
   reflect the new scene.
5. Return `bindings`.

**Explicitly not done on this path** (and why it's safe):
- No `GraphEditBatchGuard` — no graph edits happen, only param pushes.
- No `clear_rack_sampler_runtime_pools_for_track` — pools keep their voices; sounding
  voices are untouched. This is the whole point.
- No `connect_fx_chain_host` — signature equality includes the FX chain node ids, so
  the existing wiring is already correct.
- No `topology_epoch` bump — trigger topology is unchanged.

### 1.5 Logging (for verification)

Gate a decision log behind an env var, matching the codebase's existing pattern (cf.
`TINYSEQ_LOG_VOICE_COUNTS` in `ui/main.rs`):

```rust
if std::env::var_os("TINYSEQ_LOG_RACK_SYNC").is_some() {
    eprintln!("rack sync track {track_idx}: {}", if in_place { "in-place" } else { "rebuild" });
}
```

---

## Layer 2 — Deferred teardown (tail-safe rebuild)

When the signature *doesn't* match, the rebuild still runs at the boundary — and today
its first act is deleting the nodes a boundary-straddling voice is playing on. Layer 2
splits "stop using the old nodes" (immediate) from "delete the old nodes" (deferred).

### 2.1 Data

On the graph controller (or `App.graph`, wherever mutable per-frame state lives):

```rust
pub struct DeferredRackTeardown {
    pub slots: Vec<RackSlotNodeIds>,
    /// Engine ids whose per-track routes must be deleted at reap time.
    pub engine_ids: Vec<usize>,
    pub track_idx: usize,
    pub due_at: std::time::Instant,
}

pub deferred_rack_teardowns: Vec<DeferredRackTeardown>,
```

```rust
/// How long orphaned rack nodes ring out before deletion. Generous enough for
/// long release tails; a voice-idle check can replace this later.
const RACK_TEARDOWN_TAIL: std::time::Duration = std::time::Duration::from_secs(8);
/// Bound the number of orphaned generations if someone spams scene changes.
const MAX_DEFERRED_RACK_TEARDOWNS: usize = 16;
```

### 2.2 Changes to `rebuild_rack_slot_graph`

Inside the existing `GraphEditBatchGuard` block, **replace**:

```rust
for engine_id in old_engine_ids.iter().copied() {
    self.delete_engine_route_for_track(engine_id, track_idx);
}
for slot in &old_rack_slots {
    self.delete_rack_slot_nodes(slot);
}
```

**with** deferral:

```rust
if !old_rack_slots.is_empty() || !old_engine_ids.is_empty() {
    self.enqueue_deferred_rack_teardown(DeferredRackTeardown {
        slots: old_rack_slots.clone(),
        engine_ids: old_engine_ids.clone(),
        track_idx,
        due_at: Instant::now() + RACK_TEARDOWN_TAIL,
    });
}
```

**Keep** `clear_rack_sampler_runtime_pools_for_track(track_idx)` where it is, and keep
the subsequent `publish_sampler_voice_runtime` calls — the *pool bookkeeping* moves to
the new generation immediately (new triggers allocate new voices), while the old voice
*nodes* stay in the graph rendering their tails. The old slot sums remain connected to
the track's `voice_sum_id`/`voice_sum_r_id`, so the tails keep flowing to the mix; no
disconnection is needed at swap time. Verify during implementation that a sounding
sampler voice keeps rendering after its pool is cleared/republished (expected: yes —
pools are allocation bookkeeping, voices are self-contained graph nodes with their own
envelopes; this is the one behavioral assumption in this spec not yet verified in
code).

Also **move** the trailing engine-runtime reap out of the rebuild:

```rust
// DELETE this block from rebuild_rack_slot_graph:
for engine_id in old_engine_ids {
    if !self.engine_is_still_referenced(engine_id) {
        self.delete_engine_runtime(engine_id);
    }
}
```

That check now happens at reap time (2.3), after the route is actually deleted.
Deferring the `delete_engine_route_for_track` means an old engine keeps feeding the old
slot mixer during the tail window — which is exactly what lets a held engine note ring
out. (If the new scene reuses the same engine, the signature usually matched and we
never got here; if it didn't match for another reason, `connect_engine_to_track` builds
fresh route nodes for the new generation — old and new routes coexist harmlessly until
reap.)

`enqueue_deferred_rack_teardown` pushes and enforces the cap: if
`deferred_rack_teardowns.len() > MAX_DEFERRED_RACK_TEARDOWNS`, immediately reap the
oldest entry (call the reap body on it synchronously).

### 2.3 The reaper

```rust
pub fn reap_due_rack_teardowns(&mut self) {
    if self.deferred_rack_teardowns.is_empty() { return; }
    let now = Instant::now();
    let due: Vec<DeferredRackTeardown> = /* drain entries with due_at <= now */;
    if due.is_empty() { return; }
    let _batch = GraphEditBatchGuard::new(self.app.graph.lg.0);
    for teardown in &due {
        for engine_id in teardown.engine_ids.iter().copied() {
            self.delete_engine_route_for_track(engine_id, teardown.track_idx);
        }
        for slot in &teardown.slots {
            self.delete_rack_slot_nodes(slot);
        }
    }
    drop(_batch);
    for teardown in &due {
        for engine_id in teardown.engine_ids.iter().copied() {
            if !self.engine_is_still_referenced(engine_id) {
                self.delete_engine_runtime(engine_id);
            }
        }
    }
}
```

(Adapt borrow structure as needed — e.g., `std::mem::take` the vec, partition into
due/not-due, put the not-due back.) **No `topology_epoch` bump** — these nodes are
unreachable from the scheduler's perspective (bindings and pools already point at the
new generation), so deleting them changes nothing the scheduler tracks.

**Caution on `delete_engine_route_for_track`:** read its body (~4259) before wiring
this up. If it resolves route node ids through *current* per-track state (rather than
taking them as arguments), deferred invocation may target the *new* generation's
routes. In that case, capture the concrete route node ids into
`DeferredRackTeardown` at rebuild time (extend the struct with
`engine_route_nodes: Vec<i32>` or whatever the route consists of) and delete those
directly at reap time instead of calling the lookup-based helper. The same review
applies to `engine_is_still_referenced` — it must be evaluated against post-reap state,
which the ordering above already ensures.

### 2.4 Reap scheduling (call sites)

1. **UI loop tick:** in `ui/main.rs`'s main event loop, right after the
   `app.drain_due_pattern_launches()` block (~6780), call the reaper through whatever
   accessor yields the graph controller there (the same one `apply_sample_ids` is
   reached through — see how `tui/mod.rs:976` gets `self.graph_controller()`). The
   early-return on an empty vec makes this free per frame.
2. **Force-reap on structural edits:** any track add/delete/reorder invalidates the
   stored `track_idx` values. Simplest safe rule: expose
   `force_reap_all_rack_teardowns()` (same body, ignoring `due_at`) and call it at the
   top of every code path that adds/removes/reorders tracks (grep the call sites of
   `remove_track_lane_if_present` / track-push paths in `sequencer/state.rs:1977+` for
   where track topology changes, and the tui-side handlers that drive them). A cut-off
   tail during an explicit track-structure edit is acceptable; a deleted node id being
   re-deleted later is not. (Check whether `delete_node` on an already-deleted id is
   safe; assume not.)
3. **Same-track re-rebuild before reap:** needs no special handling — each rebuild
   enqueues its own generation; multiple generations of one track coexist and reap
   independently.

### 2.5 What Layer 2 deliberately does not do

- No crossfade between old and new generations. The old voices decay naturally; the
  new generation starts clean. If A/B comparison against Ableton-style device swap ever
  demands a fade, add a gain ramp on the old `slot_sum` nodes at swap time — the node
  ids are in the teardown entry.
- No voice-idle detection for early reap. The 8s timer is the v1; if node-count
  pressure ever matters, `publish_sampler_voice_runtime`-style stats can drive an
  earlier reap.

---

## Layer 3 — Prepare-ahead quantized swap (future work, design only)

Goal: for scene changes that *do* change rack topology, do the expensive build during
the quantized countdown so the boundary commit is O(reconnect), not O(build).

**Hook (already exists):** the UI loop polls
`state.quantized_launches().pending_target(QuantizedLaunchOwner::Transport)` every tick
(`ui/main.rs` ~6791, the `queued_transport_scene` logic). When a pending target scene
appears or changes, that's the prepare trigger; when it disappears or the token is
superseded (`owner_tokens` replacement in `quantized_launch.rs`), that's the
invalidation trigger.

**Prepare step** (UI thread, on enqueue): for each rack track, read the *target*
scene's `rack_track` snapshot out of the scene pool (read-only —
`scenes.track_pools[track].get(id)`), compute its `RackTopologySignature`, and compare
with the live `rack_signature` (Layer 1's cache). For mismatched tracks, run a
variant of `rebuild_rack_slot_graph` factored as `build_rack_slot_generation(...)`
that:

- creates mixers/voices/engine-routes exactly as today, **but** does not delete
  anything, does not touch `track_node_ids`, does not clear pools, and does not call
  `publish_sampler_voice_runtime` (pool bookkeeping stays on the live generation until
  commit — this is the key trick that avoids needing A/B pool banks: node building and
  pool publishing are already separate steps in today's code);
- builds the new slot mixers **muted** (create with `mute=true`) so the silent new
  generation can even be pre-connected to the voice sums, making commit purely
  param-pushes + bookkeeping;
- returns `PreparedRackSwap { token, track_idx, target_signature, slots: Vec<RackSlotNodeIds>, bindings }`.

**Commit step** (inside `apply_pattern_launch` / the rack sync path): if a
`PreparedRackSwap` exists for this launch token and its `target_signature` still equals
the incoming snapshot's signature: swap `track_node_ids[track].rack_slots` to the
prepared generation, `publish_sampler_voice_runtime` for the prepared voices, push the
real (unmuted) mixer params, run `connect_fx_chain_host` for the prepared slots, sync
bindings, enqueue the old generation as a `DeferredRackTeardown` (Layer 2), update
`rack_signature`, bump `topology_epoch` once. Otherwise fall through to the Layer 1/2
synchronous path — **prepare-ahead is only ever an optimization; correctness never
depends on it.**

**Invalidation rules:** discard (and immediately reap, ignoring the tail timer — the
prepared nodes never sounded) any `PreparedRackSwap` whose token was superseded or
cancelled, and any prepared swap for a track whose rack is edited by the user during
the countdown (all such edits funnel through the four user-edit `rebuild_rack_slot_graph`
call sites — hook there).

**Open questions to resolve when implementing Layer 3:**
- Engine slots whose engine runtime doesn't exist yet: `ensure_custom_engine_runtime`
  loads a dylib-backed engine — confirm it's safe on the UI thread mid-playback
  (it already runs there during user edits, so likely yes).
- Whether pre-connecting muted slot sums to the live voice sums measurably costs DSP
  for large racks; if so, leave the prepared generation disconnected and do the two
  `graph_connect` calls per slot at commit inside one batch guard.

---

## Test & verification plan (Layers 1+2)

**Unit tests** (pure, no audio graph — put them in a `#[cfg(test)] mod` next to the
signature code, matching the codebase's inline-test convention):

1. `signature_equal_for_identical_snapshots` — build two identical
   `RackTrackSnapshot`s (2 sampler slots), assert equal signatures.
2. `signature_ignores_parameter_fields` — clone a snapshot, change `gain`, `pan`,
   `mute`, `solo`, `max_polyphony`, `sample_id`, `pad_note`, `choke_group`,
   `param_plocks`, macros; assert signatures still equal.
3. `signature_detects_topology_changes` — one assert each for: slot count change, slot
   order swap (sampler↔engine), `instrument_type` change, `engine_id` change,
   `instrument_run_mode` change, FX chain node-id change, FX chain length change.

**Build/lint:** `cargo build` and `cargo test` for the sequencer crate (use the
workspace's standard invocation).

**Manual verification (the actual bug):**

1. Run the app with `TINYSEQ_LOG_RACK_SYNC=1`. Project: 2 rack tracks, each with two
   sampler slots; four-on-the-floor kick on one rack; two scenes with identical rack
   configs but different patterns; quantization 1/4.
2. Toggle scenes while playing. Expected: log prints `in-place` for every rack on
   every scene change; the boundary kick rings out fully; no audio glitch.
3. Change scene B's rack (different instrument in slot 1). Toggle again. Expected: log
   prints `rebuild` for that track only; the boundary kick still rings out fully
   (Layer 2); after ~8s the orphaned nodes are reaped (verify node count via whatever
   graph stats logging exists, or temporarily log in the reaper).
4. Rapid-fire scene changes across differing racks: no crash, node count bounded
   (cap kicks in), audio stays clean.
5. Regression: user edits still work live — add/remove a slot, swap a slot instrument,
   drag slot gain/pan, solo/mute, swap a slot's sample, edit the slot FX chain — both
   while stopped and during playback.
6. Regression: scene changes on *non*-rack tracks (plain sampler, custom instrument)
   behave as before (this code path is rack-gated, but confirm `apply_sample_ids`'s
   track-sampler loop is untouched).

## Implementation order (for the implementing agent)

1. Signature type + pure function + unit tests (no behavior change).
2. `rack_signature` cache field + assignment in `rebuild_rack_slot_graph` (no behavior
   change).
3. `apply_rack_scene_state_in_place` + the gate in
   `sync_live_rack_tracks_from_pattern_state` + logging. **This alone should fix the
   reported bug** — manually verify step 2 of the plan before continuing.
4. `DeferredRackTeardown` + rebuild changes + reaper + UI-loop and force-reap call
   sites. Manually verify steps 3–4.
5. Full regression pass (steps 5–6), `cargo test`, done. Layer 3 is explicitly out of
   scope.
