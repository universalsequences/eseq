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
external modulation -> modulator inputs 5..8
modulator -> synth inputs 5..8
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
- `mod1..mod4`: unipolar `0..1`

The four mod inputs are configurable host modulation slots. Each slot chooses
one source type from `off`, `lfo`, `env`, `rand`, `drift`, or one of the four
external modulation buses `ext1..ext4`.

## Native Modulator Node

The reusable modulator node provides four independent slots. Each slot owns its
own state and parameters for source classes that need state, so two slots can
both be independent LFOs or envelopes.

Output contract:

- `out 1`: `mod1`
- `out 2`: `mod2`
- `out 3`: `mod3`
- `out 4`: `mod4`

Default source mapping:

- `mod1`: LFO
- `mod2`: envelope
- `mod3`: stepped random
- `mod4`: slow drift

## Preset Model

Long-term presets should store both synth params and modulation config.

Conceptual structure:

```json
{
  "params": {},
  "mod": {
    "mod1_source": "lfo",
    "mod1_lfo_shape": "triangle",
    "mod1_lfo_rate_sync": "1/8",
    "mod2_source": "env",
    "mod2_env_mode": "AD",
    "mod3_source": "rand",
    "mod4_source": "drift"
  }
}
```

The slot source selection and source parameters live in the instrument parameter
layout so they can be saved in presets and parameter locked.

## Shared Lisp Helpers

The injected instrument preamble should define:

- input symbols for `mod1..mod4`
- safe modulation helpers

Suggested helpers:

- `mod_unipolar`
- `apply_pitch_mod_semi`
- `apply_cutoff_mod_safe`
- `apply_pw_mod_safe`
- `slew`

## Implementation Plan

## Project Compatibility

The four-slot layout intentionally breaks the older fixed ten-lane modulation
layout. Host modulation parameters use a new `MOD_PARAM_BASE`, so old saved
values are not silently interpreted as the new slot source and depth state.

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
