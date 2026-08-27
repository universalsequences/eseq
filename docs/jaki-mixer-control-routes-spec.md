# Jaki mixer-control routes: sequenced track/group mute & solo

Bead: eseq-jo7.22. Rev 1 (2026-08-26).

Jaki patterns can drive the mixer: a route whose destination is a **control
target** turns the pattern's events into timed mute/solo gestures instead of
notes. The motivating use case is performed gating — periodically muting and
soloing tracks that feed sends, so the send tails become part of the
arrangement — authored as a pattern instead of played by hand.

## 1. Surface DSL

Control routes reuse the `->` route grammar of `jak`. Because the
`(mute …)`/`(solo …)` form is unambiguous, it may sit anywhere in the
segment — `-> (shift 2) (mute 9) left` equals `-> (mute 9) (shift 2) left`;
the first control form is the target and every other item is a route word
(note routes keep destination-first). The destination is a
list headed by `mute` or `solo` instead of a bare track number:

```lisp
(jak "gate" :16
  . . - . (every 4 rev)
  -> 0                            ; ordinary note route, unchanged
  -> (mute 3)                     ; mute track 3 during event gates
  -> (mute 0) left                ; mute track 0 during left-hand gates only
  -> (solo (group "Drums")) inv)  ; solo Drums everywhere EXCEPT event gates
```

- **Targets.** A number targets a track by index (matching note routes). A
  `(group "name")` form targets a track group by name; the group's mute/solo
  is its backing bus, exactly like the mixer header buttons. Targets are
  per-cycle argument data like every other route argument: `(mute (cyc 1 2))`
  and `(solo (group (cyc "Drums" "Synths")))` rotate the destination each
  cycle, resolved via `resolve-arg` against the tick's located cycle.
- **Events are gate windows.** An evaluated event at offset `t` with gate `g`
  contributes the window `[t, t+g)`. Windows in one cycle are unioned
  (overlaps and adjacency merge), so dense patterns hold rather than flicker.
  The control is ON inside windows, OFF outside.
- **`inv`** is a route word valid only on control routes: it complements the
  union of windows within the cycle `[0, len)` — the control is ON in the
  gaps. "Mostly muted, events punch it open."
- **Everything else composes.** All pattern-transforming route words
  (`left`/`right`/`accent`, `every`, `stac`, `shift`, `fast`/`slow`, …) apply
  to the pattern before its events become windows; velocity/note words
  (`vel`, `note`) are ignored on control routes. Note that hand/accent
  filters extend gates legato-style (core spec §7), so `-> (mute 0) left`
  holds the mute from each left hit to the next surviving event; compose with
  `stac` (`left stac`) for short punches instead.

This makes mute/solo the first members of a control-route family. A future
`(vol track level)` route ("swell on accents", MSP `line~` on a fader) maps
the same event → (time, duration, value) shape and reuses this entire
pipeline; it is explicitly out of scope for this bead.

## 2. Event representation

A new scheduler-VM native emits one control hold per window that starts in
the current tick's unit window:

```
(seq-emit-control :op "mute"|"solo" :track idx | :group "name"
                  :at offset-beats :dur beats)
```

- `alez.jaki.core` computes the windows (union / complement, exact rationals)
  and converts units to beats with the recorded `jaki-unit`, mirroring
  `emit-window`.
- The native validates argument *shape* (op, exactly one target, numeric
  at/dur ≥ 0) and pushes an `EmittedMixerControl` into the generator tick
  context beside `emitted`. Malformed args follow the native error contract
  `seq-emit` uses: an error status is reported, the call returns `false`, and
  nothing is emitted.
- Carriage: `GeneratorTickResult.controls` →
  `GeneratorRuntime::process_block` resolves each to an absolute engage
  sample (boundary sample + offset beats × samples/quarter) and a release
  sample (engage + dur) → the scheduler lookahead pushes
  `ScheduledMixerControl`s into a mailbox on `SequencerState`
  (`scheduled_mixer_controls`), the same shape as the quantized-launch
  mailbox.

## 3. Application (app thread, production scheduler path)

The scheduler carries and timestamps the controls; the **app thread applies
them**, draining the mailbox once per frame in the event loop (next to
`drain_due_pattern_launches`). A due control is applied through the same code
path as clicking the mixer button:

- track mute: `track_params[t].set_mute` + `push_track_mute`
- track solo: `track_params[t].set_solo` + `push_track_solo_mutes`
  (global solo recompute)
- group mute/solo: resolve `(group "name")` → `app.groups` entry → backing
  `bus_id` → bus mute/solo + `push_bus_mute` / `push_bus_solo_mutes`

plus the matching `TrackMixer` UI invalidations, and **no undo history** —
scheduled gestures are playback, not edits.

Hold bookkeeping lives app-side, per `(op, target)`: engaging sets the flag
ON and records the release sample; an engage while already engaged extends
the release to the later time (union across ticks/routes); when the rendered
sample passes the release, the flag is set OFF. Scheduled controls *set* the
flag on engage and *clear* it on release — they deliberately stomp a manual
toggle held during the window; the manual button remains live outside
windows.

Timing is frame-drain quantized (~one UI frame of jitter). The audible gate
is a mixer param push, which is block-quantized regardless; sample-exactness
is not a goal for mixer gestures.

## 4. Determinism & failure

- **Ordering at equal times:** all due releases (OFF) apply before any due
  engages (ON), so back-to-back windows stay engaged; engages then apply in
  `(sample, generator_index, emission order)` order — authored route order
  breaks ties within one generator tick.
- **Solo vs mute** on the same target are independent flags, exactly as in
  the mixer (solo does not clear mute); both may be held simultaneously.
- **Invalid targets fail loudly, atomically, and consistently:** a track
  index out of range or an unknown group name produces a visible host error
  (status bar) at apply time and applies nothing — it can never fall through
  to a different target. Group names resolve at apply time; the resolved
  application is by the group's stable id → backing bus.
- **Transport stop / pattern switch:** pending (not yet engaged) controls are
  dropped and all engaged holds are released, so nothing stays stuck muted
  and no stale hold fires after a restart.
