# Free Patch Mode And Clock Input Spec

## Purpose

Custom DGenLisp instruments are currently hosted as sequencer-triggered synth
voices. This is correct for normal instrument workflows, but it prevents a
patch-editor workflow where the user can freely patch audio and hear the result
without first playing a note.

This spec adds:

- a saved per-instrument run mode for free-patching
- an audible idle voice for free-patch instruments
- named host-signal input binding for custom instruments
- named patch-defined modulation outputs
- a transport-synced `clock` input available to both normal instruments and
  free-patch instruments

The goal is to support writing an entire self-running patch while still keeping
sequencer gate, pitch, velocity, trigger, and modulation available as normal
signals inside that patch.

## Design Goals

- Let a patch like `50 -> phasor -> out` be audible immediately after
  compile/load/recompile in free-patch mode.
- Keep `gate` as a real sequencer signal. Free-patch mode must not mean
  "gate is forced high forever."
- Make `clock` available to regular instruments too, so instrument authors do
  not need to fake a bar ramp with a modulatable parameter and host LFO.
- Bind host signals by manifest input names/roles, not by fragile channel
  positions.
- Preserve existing clock-less instruments without requiring their mod inputs to
  move.
- Persist the mode as host metadata next to the instrument source.
- Keep DGenLisp DSP source pure DSP; host run mode remains host-owned metadata.
- Let a free-patch "god patch" expose explicit modulation outputs that can be
  patched into other tracks' `ext1..ext4` modulation inputs.
- Avoid hidden notes, fake sequencer steps, or other trigger-path workarounds.

## Non-Goals For V1

- A full song/clip launcher inside the patcher.
- Polyphonic free-patch mode.
- Arbitrary user-defined host signals.
- Polyphonic aggregation semantics for patch-defined modulation outputs.
- DAW-style transport-position APIs beyond the `clock` ramp.
- Changing DGenLisp compiler semantics except where manifest metadata is needed
  for explicit host binding.
- Moving existing `mod1`..`mod4` inputs to make room for `clock`.

## User-Facing Mode

The instrument finalization panel should expose a mode selector:

```text
Mode: Instrument | Free Patch
```

### Instrument Mode

`Instrument` is the current default behavior:

- custom instrument voices become audible when the sequencer or keyboard
  triggers them
- voice routes may remain silent until a voice is allocated
- `gate`, `pitch`, `velocity`, and `trigger` are driven by note events
- `clock`, if declared, is still available as a named host input

### Free Patch Mode

`Free Patch` means the host keeps one mono idle voice audible:

- voice 0 is routed to the track output immediately after load
- voice 0 remains routed after recompiles/hot reloads
- instrument defaults are dispatched to voice 0 immediately
- `gate` starts at `0.0`
- `trigger` starts at `0.0` and remains pulse-only
- `pitch` starts at the host's default pitch value
- `velocity` starts at `1.0`
- sequencer/keyboard events still update `gate`, `pitch`, `velocity`, and
  `trigger`
- gate-off events set `gate` back to `0.0` but do not close the voice route

In other words, free-patch mode changes idle audibility and routing, not gate
semantics.

## Saved Instrument Metadata

Run mode should be saved in a sidecar metadata file instead of encoded as a
comment in `dsp.lisp`.

Folder-style instrument:

```text
instruments/my-patch/
  dsp.lisp
  ui.lisp              ; optional
  instrument.json
```

Legacy single-file instrument:

```text
instruments/my-patch.lisp
instruments/my-patch.instrument.json
```

Recommended sidecar shape:

```json
{
  "version": 1,
  "run_mode": "free_patch"
}
```

Allowed `run_mode` values:

- `"instrument"`
- `"free_patch"`

Rules:

- missing sidecar defaults to `"instrument"`
- invalid sidecar values should report a load error and fall back to
  `"instrument"` only if the UI clearly reports that fallback
- new instruments created from the finalization panel should save this metadata
  immediately
- project save/load should also persist the resolved track run mode, so
  transient drafts and unsaved edits restore correctly

## Host Input Binding

The DGenLisp manifest already exposes declared inputs with names:

```rust
pub struct DGenInput {
    pub channel: usize,
    pub name: String,
}
```

The host should use that manifest data to bind host-provided signals by name.

### Host Signals

V1 host signals:

| Name aliases | Meaning |
| --- | --- |
| `gate` | sequencer/keyboard gate, 0 or 1 |
| `pitch` | pitch in Hz |
| `velocity`, `vel` | normalized velocity |
| `trigger`, `trig` | one-sample/block trigger pulse |
| `clock` | transport-synced bar-phase ramp, 0..1 |

Input name matching should be case-insensitive and should normalize common
separator differences where practical. For example, `clock`, `Clock`, and
`bar_clock` may map to the same host signal if the normalizer supports
underscore removal.

### Modulator Inputs

Inputs declared with `@modulator N` must continue to bind through
`manifest.modulators`, not through channel position.

Example:

```lisp
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
```

If an instrument adds `clock`, it must not force `mod1` to move:

```lisp
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def clock (in 9 @name clock))
```

This must wire correctly because `clock` is found by name and `mod1` is found
by modulator metadata.

## Patch-Defined Modulation Outputs

Free-patch workflows should be able to expose control signals from the patch
back to the rest of the project. This lets a self-running "god patch" generate
global or semi-global modulation lanes that can be patched into other tracks'
external modulation inputs from the mixer.

### Authoring Model

V1 should use the existing `@modulator N` metadata token for explicit
modulation-output declarations. Direction is determined by the operator:

- `@modulator N` on an `in` form declares a modulation input consumed from the
  host
- `@modulator N` on an `out` form declares a modulation output exposed back to
  the host

Source form:

```lisp
(out slow-ramp 2 @name macro-a @modulator 1)
(out stepped 3 @name macro-b @modulator 2)
```

This requires DGenLisp to extend `out` parsing so output-side `@modulator`
metadata is reported as modulation-output manifest data. Existing input-side
`@modulator` metadata on `in` forms must continue to populate
`manifest.modulators`.

Required metadata per modulation output:

- stable slot/index
- display name
- source output channel or generated output channel
- range convention

Suggested manifest shape:

```json
{
  "modOutputs": [
    { "slot": 1, "channel": 2, "name": "macro-a", "range": "unipolar" },
    { "slot": 2, "channel": 3, "name": "macro-b", "range": "unipolar" }
  ]
}
```

Rules:

- modulation outputs are audio-rate control signals
- V1 range is unipolar `0.0..1.0`
- the host should clamp outgoing modulation outputs to `0.0..1.0` before they
  enter destination `ext` inputs
- modulation outputs must not count as user-facing audio outputs in the mixer
- audio output routing and modulation output routing must be separate host
  concepts, even if they are both DGenLisp output channels internally

### Mixer/Patcher UI

Tracks with patch-defined modulation outputs should show one output port per
declared modulation output in the same mixer patching area where existing
modulator-track outputs appear.

For example, a free-patch track declaring:

```lisp
(out lfo-a 2 @name A @modulator 1)
(out ramp-b 3 @name B @modulator 2)
```

should expose two source circles on that track. The user can cable each source
circle to any destination track's `ext1`, `ext2`, `ext3`, or `ext4` input
circle.

UI rules:

- use the declared output name as the port label/tooltip
- preserve the existing visual language for modulation patch cables
- do not require the track to be an `InstrumentType::Modulator`
- free-patch instruments and dedicated modulator tracks should coexist in the
  same routing graph

### Routing Semantics

Existing external modulation inputs remain destination-side inputs:

```text
destination track ext1
destination track ext2
destination track ext3
destination track ext4
```

Patch-defined modulation outputs become source-side ports:

```text
source track mod output 1
source track mod output 2
...
```

The project routing model should therefore identify both:

- source track
- source modulation output index
- destination track
- destination external input index

The existing single-output modulator track can be represented as source
modulation output `0` or `1`, but the project format should not assume every
source track has only one modulation output.

### V1 Scope

Patch-defined modulation outputs are supported for:

- free-patch custom instruments
- dedicated modulator tracks
- mono custom instruments if the host can prove a single routed voice owns the
  output

Patch-defined modulation outputs are not supported for polyphonic custom
instrument voices in V1. A polyphonic instrument that declares modulation
outputs should either:

- hide/disable those modulation output ports with a clear diagnostic, or
- require the instrument track to be in free-patch/mono mode before routing
  them

This avoids inventing fragile aggregation semantics such as "sum all voices" or
"use the newest voice" without a strong musical model.

Future versions may add explicit aggregation modes:

- `max`
- `sum-clipped`
- `average-active`
- `voice-0`

Those modes should be declared in metadata before polyphonic modulation outputs
are routable.

### Relationship To Free Patch Mode

Free-patch mode is the primary V1 use case:

- the idle voice is always routed
- modulation outputs are continuously produced
- the output signal can sequence or animate other tracks even when no note is
  playing

Regular `Instrument` mode can still declare modulation outputs for future use,
but the host should not expose routable ports unless the output has a clear
single-voice owner.

### Legacy Fallback

For manifests that lack names, the host may preserve the existing convention as
a compatibility fallback:

- input channel 0 -> `gate`
- input channel 1 -> `pitch`
- input channel 2 -> `velocity`
- input channel 3 -> `trigger`

This fallback must not override explicit names or `@modulator` metadata.

## Transport Clock Semantics

`clock` is an audio-rate control signal generated by the host.

V1 definition:

- value range: `0.0 <= clock < 1.0`
- phase: bar phase
- one cycle: 4 quarter-note beats
- tempo source: project transport BPM
- reset: phase resets to `0.0` on transport start/seek
- stopped transport: clock holds at `0.0`
- running transport: clock advances continuously at audio rate

At 120 BPM, one bar is 2 seconds, so `clock` completes one `0..1` ramp every
2 seconds.

Future host signals may add `beat_clock`, `sixteenth_clock`, `bpm`, or
`playing`, but V1 should implement only `clock` unless a concrete patching need
requires more.

## Runtime Architecture

### Current Relevant Pieces

Current custom instruments use:

- `gatepitch` node outputs: gate, pitch, velocity, trigger
- `voice_modulator` outputs for `@modulator` slots
- per-engine custom instrument voices
- per-track route gain nodes for engine voice output routing
- route gains initialized silent until `route_custom_voice_to_track(...)`
  opens them for a triggered voice

This is why a constant oscillator patch can be silent until the first note:
the DSP can be valid, but no custom engine voice is routed to the track yet.

### Required Runtime Changes

Add a runtime run mode for custom instrument tracks:

```rust
enum CustomInstrumentRunMode {
    Instrument,
    FreePatch,
}
```

The mode must exist in:

- saved instrument metadata
- project snapshot/project file data
- live runtime track state
- UI state shown in the instrument finalization panel

When a custom instrument track enters `FreePatch` mode:

- ensure the custom engine runtime exists
- route engine voice 0 to the owning track
- set route gains for voice 0 to `1.0`
- dispatch instrument defaults to voice 0
- keep gatepitch state at idle values:
  - gate = `0.0`
  - pitch = default host pitch
  - velocity = `1.0`
  - trigger = `0.0`
- mark or track this voice as the free-patch idle voice without pretending a
  musical note is active

When the mode is disabled or the track changes back to `Instrument`:

- close any free-patch idle route that was opened solely by the mode
- do not send fake note-off events
- leave ordinary active notes/release tails owned by normal scheduling logic

### Note And Sequencer Events In Free Patch Mode

For V1, free-patch mode is mono:

- sequencer and keyboard note events should target voice 0
- note-on updates pitch, velocity, gate, and trigger
- note-off sets gate to `0.0`
- route remains open after note-off
- p-lock/default parameter dispatch applies to voice 0

If an instrument is set to polyphonic while in free-patch mode, the host should
either:

- ignore polyphony and show mode as mono, or
- disable free-patch mode with a clear diagnostic

V1 should choose the simpler rule: free-patch mode is mono and ignores
polyphonic allocation.

## Graph Wiring

Replace positional host-input wiring with a binding pass:

```text
for each manifest input:
  if input is listed in manifest.modulators:
    connect matching voice_modulator output
  else if normalized input.name matches a host signal:
    connect host signal output to input.channel
  else:
    leave unconnected or connect silence according to existing graph behavior
```

This requires a host signal source with at least:

- gate output
- pitch output
- velocity output
- trigger output
- clock output

The current `gatepitch` node can either be extended or replaced by a more
general host-input node. The robust design is to introduce an
`instrument_host_inputs` node so `clock` is not conceptually part of
gate/pitch.

Suggested outputs:

```text
0 gate
1 pitch
2 velocity
3 trigger
4 clock
```

If the existing `gatepitch` node is extended, it should be renamed or wrapped so
the code no longer treats it as only gate/pitch.

## Patcher Defaults

New instrument templates should include `clock` as an available named input,
but existing instruments should not be rewritten.

Suggested default instrument source:

```lisp
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
```

This ordering is only a template convention. Runtime correctness must come from
named/role binding.

## UI Requirements

In the finalization panel:

- show the mode selector near the instrument name/save controls
- default to `Instrument`
- persist the selected mode when finalizing
- if the selected mode is `Free Patch`, the current draft track should become
  audible immediately after compile succeeds

In the existing instrument UI:

- show current mode somewhere compact in the instrument source/synth area
- allow changing mode for an existing custom instrument track
- changing mode should update live runtime routing without requiring a reload

## Testing Requirements

### Unit Tests

- parse sidecar metadata:
  - missing file -> `Instrument`
  - `"instrument"` -> instrument mode
  - `"free_patch"` -> free-patch mode
  - invalid mode reports a useful error
- manifest input binding:
  - `clock` binds by name even when declared after `mod1`
  - `mod1` binds through `@modulator 1` even if `clock` appears before it
  - legacy unnamed channels still bind gate/pitch/velocity/trigger
- modulation output metadata:
  - declared modulation outputs appear in the parsed manifest with stable names
    and slots
  - modulation outputs do not increase the user-facing audio output count
  - project routing preserves source output index and destination ext input
- project snapshot/save/load preserves run mode

### Audio/Runtime Tests

Use `instrument_probe` or a similarly direct host-path test when possible.

Required signals:

- a free-patch instrument containing only `(out (phasor 50) 1 @name audio)`
  produces nonzero output without a note trigger
- the same source in `Instrument` mode remains silent until routed/triggered
- in free-patch mode, a patch that outputs `gate` is silent at idle and changes
  only when sequencer/keyboard gate changes
- hot reload preserves free-patch audibility
- `clock` produces a monotonic `0..1` ramp while transport is playing
- `clock` resets to `0.0` on transport start/seek
- a free-patch modulation output produces a clamped `0..1` control signal at a
  destination track's selected `ext` input
- hot reload preserves modulation output routes when output identities are
  stable

### UI/Layout Tests

When adding the mode selector:

- verify the selector text/control has finite, nonzero measured rects inside
  the finalization panel
- verify the control is reachable and can toggle modes
- verify saving in each mode writes the expected sidecar metadata
- verify declared modulation output ports have finite, nonzero measured rects
  in the mixer patching area
- verify a cable can connect a patch-defined modulation output to a destination
  `ext` input

## Implementation Sequence

1. Add sidecar metadata load/save helpers and tests.
2. Add run mode to project/runtime track state.
3. Add named host-input binding from manifest inputs.
4. Add or replace the host input node with a `clock` output.
5. Add DGenLisp/manifest support for patch-defined modulation outputs.
6. Extend modulation routing state to include source output indices.
7. Expose patch-defined modulation output ports in the mixer patching UI.
8. Implement free-patch idle voice routing for custom instruments.
9. Make note/gate events target the idle voice in free-patch mode.
10. Add UI selector and persistence through finalization.
11. Add runtime/audio tests, then UI/layout tests.

## Fragility To Avoid

Do not implement free-patch mode by:

- injecting a hidden sequencer step
- sending a fake note that never releases
- forcing `gate` high
- relying on `clock` being input channel 5
- shifting existing mod inputs to make room for `clock`
- treating ordinary audio outputs as modulation outputs by channel-number
  convention
- exposing polyphonic modulation outputs without explicit aggregation
  semantics
- encoding run mode as an unparsed comment in `dsp.lisp`

The robust boundary is:

- DSP source declares named inputs
- manifest reports inputs and modulator metadata
- manifest reports modulation outputs as explicit control outputs
- host binds known host signals by name/role
- host routes declared modulation outputs to destination external modulation
  inputs by source output identity
- host metadata declares how the instrument should be run
