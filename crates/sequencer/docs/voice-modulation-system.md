# Voice Modulation System

## Goal

Make modulation a reusable per-voice subsystem so instruments stop re-implementing LFOs,
secondary envelopes, random sources, smoothing, and clock-derived behavior.

The synth engine should consume a fixed set of control inputs and focus on synthesis.

## Architecture

Per allocated voice:

```text
gatepitch -> synth inputs 1..4
gatepitch -> modulator inputs 1..4
modulator -> synth inputs 5..10
```

Each voice instance gets:

- one `gatepitch` node
- one `voice_modulator` node
- one synth voice node

Tracks may still share an engine pool. The modulator state lives with the allocated voice,
not with the track globally.

## Fixed Instrument Input Contract

All instruments should assume these inputs:

- `in 1`: `gate`
- `in 2`: `pitch_hz`
- `in 3`: `velocity`
- `in 4`: `trigger`
- `in 5`: `mod1`
- `in 6`: `mod2`
- `in 7`: `mod3`
- `in 8`: `mod4`

Signal conventions:

- `gate`: `0/1`
- `pitch_hz`: Hz
- `velocity`: `0..1`
- `trigger`: pulse on note-on
- `mod1..mod4`: bipolar `-1..1`
- `clock_phase`: unipolar `0..1`
- `clock_pulse`: `0/1`

## Native Modulator Node

The reusable modulator node should eventually provide:

- `lfo1`
- `lfo2`
- `env2`
- `rand`
- `drift`
- `clock_phase`
- `clock_pulse`

MVP output contract:

- `out 1`: `mod1`
- `out 2`: `mod2`
- `out 3`: `mod3`
- `out 4`: `mod4`
- `out 5`: `clock_phase`
- `out 6`: `clock_pulse`

MVP internal source mapping:

- `mod1`: cyclic modulation
- `mod2`: secondary envelope
- `mod3`: stepped random
- `mod4`: slow drift
- `clock_phase`: cycle phase
- `clock_pulse`: note trigger placeholder until transport clock wiring lands

## Preset Model

Long-term presets should store both synth params and modulation config.

Conceptual structure:

```json
{
  "params": {},
  "mod": {
    "lfo1_shape": "triangle",
    "lfo1_rate_sync": "1/8",
    "env2_mode": "AD",
    "rand_mode": "s&h",
    "mod1_source": "lfo1",
    "mod2_source": "env2",
    "mod3_source": "rand",
    "mod4_source": "drift"
  }
}
```

MVP can hardcode source assignment in the node and add preset-level routing later.

## Shared Lisp Helpers

The injected instrument preamble should define:

- input symbols for `mod1..mod4`, `clock_phase`, `clock_pulse`
- safe modulation helpers

Suggested helpers:

- `mod_unipolar`
- `apply_pitch_mod_semi`
- `apply_cutoff_mod_safe`
- `apply_pw_mod_safe`
- `slew`

## Implementation Plan

### Phase 1

- extend the instrument input contract to 10 inputs
- add a native `voice_modulator` node
- wire one modulator per voice in the graph
- expose modulation inputs in the instrument preamble

### Phase 2

- add modulator config structs and preset serialization
- store per-track modulation settings alongside instrument presets
- add runtime parameter updates for modulator nodes

### Phase 3

- add transport-derived clock wiring to the modulator node
- expose per-preset clock divisions and retrigger behavior
- add UI for LFO/env/random source configuration

### Phase 4

- refactor engines to rely on external modulation buses
- start with `DigiPRO`, then `FM+`, then `SID`, then `SuperWave`

## Current MVP Scope

This first implementation intentionally does not yet include:

- transport-synced divisions
- preset-driven mod source selection
- modulation UI
- persistent modulator presets

It establishes the reusable voice-modulation contract and graph topology first.
