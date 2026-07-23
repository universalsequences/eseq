# Audio Module Split & Reorganization Plan

Companion to `docs/big-file-split-plan.md` (same ground rules: one file per
commit, `cargo check` + scoped tests after each, never touch a file with
uncommitted local edits, preserve caller paths via re-exports).

Targets (line counts as of 2026-07-22):

| File | Lines | Role |
|---|---|---|
| `src/audio.rs` | 8,777 | real-time audio callback: voice pools, param/plock resolution, note firing, scheduling, render. ~1,650 lines (19%) is `#[cfg(test)] mod tests` (lines 7121–8777) |
| `src/engine.rs` | 426 | construction/lifecycle wrapper: builds the live graph + buses, starts workers, wraps the CPAL stream |
| `src/audiograph.rs` | 274 | FFI bindings to the C audiograph library |

The relationship is exactly as suspected: `engine` constructs, `audio` runs the
callback, both sit on `audiograph`. They become one directory module.

## Key findings that shape the plan

1. **`audio.rs` has almost no public surface.** Only three `pub` items —
   `build_output_stream`, `query_device_config`, `FALLBACK_SAMPLE_RATE` — and
   the only consumer of any of them is `engine.rs` (`FALLBACK_SAMPLE_RATE` is
   dead pub, used by nobody). Zero `crate::audio::` references exist anywhere
   else in the workspace. The 8.7k-line file is a sealed box; we can carve it
   freely with `pub(super)`/`pub(crate)` visibility and no external breakage.

2. **`engine`'s consumers are all binary targets**, reached via the external
   `sequencer::engine::…` path: `ui/main.rs` (`init_engine`), `ui/capture.rs`,
   `ui/tests.rs`, `bin/graph_resource_probe.rs` (`init_headless_engine`).
   Nothing lib-internal uses it. Public contract: `Engine`, `HeadlessEngine`,
   `init_engine`, `init_headless_engine`, `destroy`.

3. **`audiograph` is the wide-open one**: referenced from 60+ files (all of
   `app/graph/*`, `app/effects.rs`, every `effects/*.rs` via `NodeVTable`,
   `sampler.rs`, `track_modulator.rs`, `voice_modulator.rs`, ~15 `ui/*` +
   `bin/*` files via the hardcoded `sequencer::audiograph::…` path). Heaviest
   items: `delete_node` (80 refs), `graph_connect` (65), `params_push_wrapper` /
   `ParamMsg` (51 each), `graph_disconnect` (49). Its entire pub surface must
   survive at the old paths.

4. **Path-stability mechanism already has precedent**: `lib.rs:67` does
   `pub use runtime::{accumulator, generator, graph, process};` to keep old
   crate-root paths working after the runtime/ reorg. We do the same:

   ```rust
   pub mod audio;
   pub use audio::{audiograph, engine};
   ```

   This keeps every `crate::audiograph::…`, `sequencer::audiograph::…`, and
   `sequencer::engine::…` call site compiling with **zero edits**.

5. **`AudioCallbackData` is the hub.** ~45 fields; nearly every free function
   in `audio.rs` takes `&mut AudioCallbackData`. Whatever file holds it is a
   dependency of every submodule — put it in `audio/state.rs` and let
   submodules take it by reference. No `static mut` seams exist in the file
   (the mutable statics live in `voice_modulator` / `SequencerState` atomics),
   so the split is purely a visibility/module exercise.

6. **One intentional mutual dependency**: the scheduled-event queue machinery
   (`dispatch_scheduled_event`, `dispatch_block_events`) calls down into
   `fire_resolved` / `dispatch_chop_event`, while the fire path schedules
   gate-off/chop events back into the queues. `events.rs` and `fire.rs` will
   reference each other — fine within one directory module, don't fight it.

## Target layout

```
src/audio/
  mod.rs             module decls, shared consts, pub use re-exports        ~120
  audiograph.rs      FFI layer, moved verbatim                              ~274
  engine.rs          Engine/HeadlessEngine + init_*, moved verbatim         ~426
  device.rs          OutputDeviceConfig/FormatRange, select_output_*,
                     env_flag (CPAL config selection)                        ~90
  stream.rs          build_output_stream, query_device_config
                     (drop to pub(crate) — engine is now a sibling)         ~200
  state.rs           AudioCallbackData, MetronomeState, ActiveKeyboard*,
                     FreePatchTransportRoute*, BusGateClock                 ~250
  graph_dispatch.rs  unsafe push_*/send_*/dispatch_*_to_voice wrappers,
                     HostTransportClock, modulator param routing            ~700
  voice_pool.rs      CustomEnginePool + allocation/stealing, pool/topology
                     sync, free-patch route sync, release + mute groups     ~950
  params.rs          plock/key-lock identity, instrument/sampler/rack
                     param resolution, sound fingerprints                   ~950
  events.rs          GateOff/Chop/Countdown/Block event types, swing,
                     countdown+block queues, scheduled-event dispatch       ~700
  rack.rs            rack slot resolution, macros, choke groups,
                     note-offs, fire_rack_* / fire_live_keyboard_rack_note ~1,050
  fire.rs            fire_resolved (~720 lines), chop dispatch,
                     keyboard-voice bookkeeping                            ~1,000
  render.rs          render_chunk, metronome mix, peak metering, bus-gate
                     sync, transport-clock / dj-mixer sync                  ~500
  callback.rs        audio_callback (~670 lines)                            ~700
  tests.rs           the whole #[cfg(test)] mod tests, verbatim           ~1,650
```

Visibility: everything internal is `pub(super)` (or `pub(crate)` where
`tests.rs`/future needs demand). External surface after the split:
- `audio::engine::{Engine, HeadlessEngine, init_engine, init_headless_engine}`
  (re-exported at crate root as `engine`)
- `audio::audiograph::*` unchanged (re-exported at crate root as `audiograph`)
- Nothing else. `FALLBACK_SAMPLE_RATE` becomes private (dead pub today);
  `build_output_stream` / `query_device_config` drop to `pub(crate)`.

## Execution phases (one commit each)

**Phase 0 — form the directory (pure `git mv`, no content edits).**
`git mv src/audio.rs src/audio/mod.rs`, `git mv src/engine.rs
src/audio/engine.rs`, `git mv src/audiograph.rs src/audio/audiograph.rs`.
In the new `mod.rs` add `pub mod audiograph; pub mod engine;` at the top.
In `lib.rs` replace the three `pub mod` lines with
`pub mod audio; pub use audio::{audiograph, engine};`.
All `crate::audiograph`/`crate::engine`/`sequencer::…` paths keep working via
the re-exports; `engine.rs`'s `use crate::audio;` and `audio`'s
`use crate::audiograph::*;` also still resolve. `cargo check` should pass with
no other edits.

**Phase 1 — extract tests.** Move `mod.rs` lines 7121–8777 (`#[cfg(test)]
mod tests`) verbatim into `src/audio/tests.rs`; leave `#[cfg(test)] mod
tests;` behind. `use super::*;` continues to see every private item because
tests is a child of the `audio` module. mod.rs drops 8,777 → ~7,130.
Run the audio-scoped tests (`cargo test -p sequencer --lib audio::`).

**Phase 2 — carve mod.rs into submodules, leaf-first.** Order chosen so each
commit moves items whose dependencies already moved (or stay in mod.rs):
1. `device.rs` (self-contained; 7 output-config tests exercise it)
2. `graph_dispatch.rs` (leaf: only depends on audiograph + state types)
3. `state.rs` (AudioCallbackData + small state structs)
4. `voice_pool.rs`
5. `params.rs` (move the ~50 live_*/snapshot_* helpers as ONE unit — they're
   a tight web; don't split the pairs)
6. `events.rs`
7. `rack.rs`
8. `fire.rs`
9. `render.rs`
10. `callback.rs` + `stream.rs` (what's left; mod.rs shrinks to decls/consts)

To keep `tests.rs`'s `use super::*;` working as items leave mod.rs, add
`pub(crate) use` re-exports in mod.rs for each submodule's items (glob
`pub(super) use device::*;` style) — mirrors how the flat file looked to the
test module, zero test edits.

**Phase 3 (optional, separate effort) — split the giant functions.**
`fire_resolved` (~720 lines), `audio_callback` (~670 — its inline keyboard
note-on/off block at ~6328–6618 duplicates fire_* logic and wants a
`handle_keyboard_triggers` extraction), `fire_live_keyboard_rack_note` (~280),
`fire_rack_slot_note`/`fire_rack_resolved` (~215/220), `dispatch_chop_event`
(~215), `CustomEnginePool::allocate_voice` (~150). Behavior-preserving but
not mechanical — do after the module split settles, if at all.

## Non-goals / deliberately left in place

- `effects/`, `track_modulator.rs`, `audio_tap.rs`, `voice_modulator.rs` are
  audiograph-node *consumers*, not callback internals — they stay put and
  import `crate::audiograph` as before.
- `recorder.rs` (126 lines) is a shared type read by 12 files across `app/*`
  and `ui/*`; owned-by-engine but not engine-internal. Stays at root.
- `voice.rs` / `scheduled_event.rs` / `sampler.rs` are inputs to the callback,
  used well beyond it. Stay at root (candidates for a later loose-file pass).

## Follow-on: `scheduler.rs` + `scheduled_event.rs` → `src/scheduler/`

`scheduler.rs` is a sixth god-file: **13,073 lines**, of which ~5,370 (41%) is
the `#[cfg(test)] mod tests` at lines 7702–13073. Its entire public surface is
one function, `spawn_scheduler_thread`, whose sole caller is
`audio/stream.rs:122`. It does NOT belong inside `audio/`:

- `scheduler` is the **producer** — a dedicated thread that walks the timeline
  snapshot and resolves steps/plocks/midi-fx/neural/generator/process
  emissions into `ScheduledEvent`s pushed onto the queue.
- `audio/events.rs` is the **consumer** — callback-side drain of that queue
  into the countdown/block machinery and `fire.rs`, welded to
  `AudioCallbackData`. It stays in `audio/`.
- `scheduled_event.rs` (527 lines) is the **contract** between the two, used
  by 8+ files (`audio/*`, `neural.rs`, `runtime/process.rs`, `lisp_host`).

Plan:
1. `src/scheduler/` directory module: `scheduler.rs` → `scheduler/mod.rs`,
   plus `git mv scheduled_event.rs` → `scheduler/scheduled_event.rs` (the
   scheduler owns the event vocabulary it emits). lib.rs compat re-export:
   `pub use scheduler::scheduled_event;` keeps all `crate::scheduled_event::`
   paths working.
2. Phase 1: extract tests → `scheduler/tests.rs` (13.1k → 7.7k).
3. Phase 2 carve by the visible clusters: snapshot clock state
   (`SnapshotSequencerClock`), snapshot-side param/plock resolution
   (`resolve_*_params/defaults/plocks`), midi-fx chain machinery
   (`midi_fx_*`), neural/generator/graph emission merging + runtime
   reconciliation, note-span/trigger geometry, and the
   `enqueue_*<const QUEUE_CAP>` pipeline + thread loop.

Known duplication (do NOT try to fix during the mechanical split):
`scheduler.rs` has private copies of `swing_delay_samples`,
`slot_param_identity`, `plock_identity_matches`, `resolved_slot_param_value`,
`ceil_to_grid`, `instrument_sound_fingerprint` that parallel the live-side
versions in `audio/params.rs`/`audio/events.rs`/`audio/render.rs` — snapshot
vs live variants with different signatures. Unifying them is a separate,
behavior-risky refactor; the split just makes the duplication visible.

## Gotchas checklist

- [ ] `audiograph.rs` has `#[cfg(test)] pub fn initialize_engine_for_test`
      used by 3 files — verify those paths after the move (they go through the
      crate-root re-export, so they should be untouched).
- [ ] Binary targets (`ui/main.rs` bin `metal_seq`, `src/bin/*`) use
      `sequencer::…` paths — covered by the lib.rs re-export, but they're a
      separate compilation unit: run `cargo check -p sequencer --bins` too.
- [ ] 17 known pre-existing metal_seq layout-test failures — don't chase them.
- [ ] No package-wide `cargo fmt`; format only moved files.
