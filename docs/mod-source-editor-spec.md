# Modulation source editor rework (drift, phase, waveform display)

Status: rev 1, slices 1–4 built 2026-09-02. Follow-ups tracked under the
`eseq-modsrc` epic.

## Problems (as reported)

1. **Drift looked broken.** `modN_drift_rate` ranged 0.00001–0.01 Hz with a
   2-decimal display, so the picker showed `0.00`, the max value was `0.01`,
   and the engine's target was white noise scaled by 0.08 through a one-pole
   whose coefficient was the rate itself. Output amplitude was a few percent
   at any rate.
2. **Dead controls.** With sync off the division dropdown rendered empty; with
   a non-pulse shape the pulse-width knob still showed. The host already
   filters the section's params (`selected_source_param_indices`) — the Lisp
   editor drew every widget regardless and got `nil` params.
3. **No phase control** for the LFO.
4. **No picture** of the LFO shape or where the cycle is.
5. (Later) editable custom LFO curves.

## Engine (`crates/sequencer/src/instruments/voice_modulator.rs`)

### Drift

Rate range `DRIFT_RATE_MIN_HZ..DRIFT_RATE_MAX_HZ` = 0.02–8 Hz (exponential,
default 0.5 Hz; sync mode uses the division's rate directly). The source
picks a new random target in −1..1 every `1/rate` seconds and glides to it
with a raised cosine (`drift_value`), so the range is full and the amplitude
is independent of rate. Per-slot state grew by two words
(`IDX_DRIFT_PHASE`, `IDX_DRIFT_TARGET`; `SLOT_STATE_STRIDE` 8 → 10, still
below `PARAM_SLOT_BASE`). Velocity no longer scales drift.

Saved projects with the old rate clamp to 0.02 Hz on load; there is no
migration because the old control produced no audible motion.

### LFO phase offset

`modN_lfo_phase`, 0–360°, linear, default 0. Applied on top of the running
phase in both free and synced modes (`lfo_effective_phase`). Retrigger still
resets the *running* phase to 0, so a 90° offset with retrigger starts each
note at 90°.

**Retrigger in sync mode** (user report 2026-09-02: "retrig ON does nothing").
The synced branch derived the phase straight from the bar position and never
looked at retrigger. Now, with retrigger on, note-on captures the bar-derived
phase into `IDX_LFO_SYNC_ANCHOR` and the slot runs `bar_phase - anchor`, so the
cycle restarts at the note while still advancing at the division's rate;
turning retrigger off re-locks to the bar. Free-run retrigger was verified in
`retrigger_tests` (gate rise and mid-block trigger pulse both reset).

**Layout contract.** The param is *appended after the slot block* at node index
`PARAM_LFO_PHASE_BASE + slot`, not inserted into the per-slot stride. Both
persistence paths are positional: `EffectSlotSnapshot::sync_to_descriptor`
preserves `min(old, new)` values by index, and
`project_slot_into_synced_snapshot_with_modulator` counts non-generated params
positionally when generated depth lanes are present. Changing
`PARAM_SLOT_STRIDE` would remap every later slot's saved values onto the wrong
control. The project loader now also matches any `MOD_PARAM_BASE` param by node
index in the generated-lanes branch, so a control the saved layout never had
takes its default rather than a depth lane's value.

### Display tail

`STATE_DISPLAY_SLOT_PHASE + slot` joins `STATE_DISPLAY_SLOT_VALUE`: the LFO's
effective phase, rand/drift's 0..1 progress to the next value, or
`DISPLAY_PHASE_NONE` (−1). Written once per block by
`publish_slot_display_values`, including the all-off and disabled-voice early
returns, so a marker never freezes.

## Host poller (`ui/state_values/meters_and_modulation.rs`)

`EffectModValues`, `InstrumentModValues`, `RackSlotModValues` carry
`slot_phases: [f64; 4]` (quantized to 1/128 like the track modulator phase).
Fields, keyed by 1-based slot number to match the panel's `:slot`:

| panel | field |
|---|---|
| effect (track/bus) | `fx-mod-slot-phase-{node_id}-{slot}` |
| FX-tile instrument | `fx-instrument-mod-slot-phase-{slot}` |
| track-keyed instrument | `instrument-mod-slot-phase-{track}-{slot}` |
| rack slot instrument | `rack-mod-slot-phase-{track}-{slot_idx}-{slot}` |

Sampling gate change: the selected track's instrument, the selected rack
slot, the selected track's chain effects, and bus effects are sampled while
the panel is live **even with nothing routed**, so the marker moves while the
LFO is being designed. Other tracks' effects keep the old "only when
modulated" gate. Rack *effects* are not sampled (never were) and get no
`phase-field`.

Each section map from the six panel builders carries `"phase-field"`; the
Lisp editor binds it with `bind-seq` and passes −1 when absent.

## Widget (`eseqlisp/src/widget_render/lfo_curve.rs`)

`lfo-curve` props: `shape` (0 triangle, 1 sine, 2 pulse, 3 saw — the
modulator's `shape_labels()` order), `pw`, `phase-offset` (degrees), `phase`
(cycles, < 0 hides the marker), `background-color`, `grid-color`,
`curve-color`, `fill-color`. One cycle of *running* phase on the x axis; the curve is
`shape(x + offset)` so the phase knob visibly slides the waveform, and the
marker dot sits at `phase - offset` on that curve. One faint zero line, fill
to zero, no other grid (user note: keep it clean, dot only). MSL in the widget file, WGSL twin in `wgsl.rs`; capture scene
`widget-lfo-curve` draws one instance per shape.

## Lisp editors

`instrument-modulation.lisp` / `effect-modulation.lisp` `lfo-source-editor`:
left column `[rate | division] [sync]`, `[shape] [phase°]`, `[retrig] [pw?]`;
right: the curve. `division` is drawn only when the param exists (sync on),
`rate` otherwise; `pw` only for pulse. The panel uses `ui-lego-panel-s 7.0`
(now exported) instead of the 5.58-tall medium readout panel.

## Follow-ups

- Custom LFO curves: `lfo-curve` grows an editable breakpoint/table mode and
  the engine gains a `shape = custom` reading a per-slot tensor. Draw the same
  widget so editing happens in place.
- Rand/drift trace: a rolling history of the slot output (the display value
  already exists) instead of only the progress marker.
- Effect LFO editors on rack effects have no marker (rack effect modulators
  are not sampled).

## Rev 2 additions (user notes, 2026-09-02)

- **Slow synced cycles.** `SyncDivision` grew `TwoBars`, `FourBars`, `EightBars`
  (labels "2 bars", "4 bars", "8 bars"; 8/16/32 beats), appended so saved
  division indices keep meaning. Every division dropdown that uses
  `SyncDivision::ALL` (LFO/rand/drift sources, delay time) gets them.
- **Triangle skew.** `shape_value` triangle takes its peak position from
  `modN_lfo_pw`: 0.5 symmetric, lower = fast rise / slow fall. The host filter
  shows the pw control for pulse *and* triangle; the editor labels it "peak"
  for triangle and "pw" for pulse. The widget draws the same skewed triangle.
- **Retrigger report.** Still reported as not restarting audibly or in the
  visualizers after the sync-mode fix. A graph-level test
  (`voice_modulator_retriggers_from_gatepitch_note_on_block_events`) drives a
  real note-on block event through gatepitch into the modulator; see the
  session report for its result.
- **Effect modulators have no gate.** `effect_chain_graph.rs` and
  `app/effects.rs` wire only the Ext inputs (ports 4–7) and the slot outputs
  into an effect-chain modulator; gate/pitch/velocity/trigger (ports 0–3)
  stay silent. So retrigger and the env source can never fire on an effect.
  The effect LFO editor no longer shows retrig. Making effect modulators
  note-aware (e.g. from the track's gatepitch) is a follow-up.
- **Env editor.** `env-source-editor` is now a full-width `adsr-editor`
  (passing `attack-max`/`decay-max`/`release-max` from the params) with the
  four number pickers underneath; the inert "env" badge is gone. Envelope
  stage ceiling `ENV_TIME_MAX_MS` = 20 s for attack, decay and release (was
  2 s / 4 s / 4 s), applied in both the descriptors and the render clamps.
- **Multi-bar sync no longer restarts per bar.** The transport only supplies
  a within-bar phase, so the modulator now counts bar wraps
  (`IDX_PREV_BAR_PHASE` / `IDX_BAR_COUNT`, reset with the mod reset counter)
  and `synced_phase_from_bar_phase` folds `bar_count mod cycle_bars` in for
  divisions longer than a bar. The cycle is anchored to the last reset
  (transport start / bar resync), not to absolute song bars.
