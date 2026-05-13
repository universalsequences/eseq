# Analog Bread-and-Butter Synth Spec

## Purpose

Define a reference DGenLisp instrument family for agent-generated custom
instruments and presets.

The goal is not to clone Ableton Analog by name. The goal is to capture the
same production virtues:

- immediate useful sound from ordinary subtractive synthesis
- musically constrained controls
- reliable gain staging
- expressive velocity, keyboard, envelope, and LFO behavior
- preset roles that sound finished without relying on external effects

Existing instruments such as `minimoog-lad2`, `korg1`, and the current
Prophet-style experiments are useful cautionary references: they expose parts
of a subtractive signal path, but the resulting patches can still feel flat
when the architecture does not enforce motion, performance response, tone
shaping, and level discipline.

## Product Target

This synth should be the default "make me a synth" target for agent mode unless
the user asks for a specific synthesis method.

It should cover:

- bass
- lead
- pluck
- stab
- pad
- soft key
- brass/string-style poly patch
- simple sound-effect patch

It should not try to cover:

- accurate historical circuit emulation
- sample playback
- full wavetable synthesis
- complex FM
- drums
- physical modeling

Those belong in separate instrument families.

## Core Design Rule

The instrument must sound good when the preset is simple.

Do not depend on dense modulation, extreme values, or external effects to make
the synth interesting. A single held note with a basic bass, pluck, or pad
preset should already have a finished tone: stable pitch center, shaped attack,
controlled brightness, non-brittle highs, appropriate body, and no accidental
clipping.

## Voice Architecture

### Inputs

Required voice inputs:

- `gate`
- `pitch`
- `velocity`
- `trigger`
- six host modulation buses, using the existing DGenLisp `@modulator` pattern

### Oscillator Section

Two primary oscillators plus sub and noise.

Oscillator requirements:

- anti-aliased saw
- anti-aliased pulse
- triangle or sine-like low-harmonic source
- continuous shape blend, not only hard waveform switching
- octave and semitone controls
- fine detune in cents
- pulse width
- per-oscillator level
- small optional oscillator slop/drift

Sub oscillator requirements:

- derived from oscillator 1 pitch
- one octave down
- pulse or sine/triangle flavor
- independent level
- capped low enough that presets cannot become accidentally unusable

Noise requirements:

- independent level
- simple color control using filtering
- route through the main tone path, not directly to output by default

Design note:

Raw oscillator sums are not the product. The oscillator section should produce
a shaped source for the filters, with level compensation and mild nonlinear
behavior before the filter.

### Mixer And Pre-Filter Tone

The mixer must be explicitly gain-staged.

Requirements:

- normalize or attenuate the sum based on active oscillator levels
- provide pre-filter drive as a musical control
- use mild saturation before the first filter
- keep noise and sub from bypassing the same tone-shaping path
- avoid full-scale oscillator summing

Recommended controls:

- `osc_mix`
- `osc1_level`
- `osc2_level`
- `sub_level`
- `noise_level`
- `pre_drive`
- `pre_tone`

### Filter Section

Minimum viable version:

- one rich low-pass filter path
- key tracking
- velocity-to-filter
- filter envelope amount
- resonance
- drive/saturation

Preferred version:

- two filters with serial/parallel blend
- filter 1 and filter 2 can each be low-pass-like in V1 if true multimode
  support is not available
- independent cutoff offsets or ratios
- shared resonance/tone controls if a smaller surface is desired

Filter requirements:

- cutoff must be modulated in Hz with clipping
- key tracking must be available and useful over normal keyboard range
- resonance must be bounded so high values do not destroy level or stability
- filter drive must change tone without causing accidental clipping
- envelope amount must support positive and negative movement

Recommended controls:

- `cutoff`
- `resonance`
- `filter_env_amt`
- `keytrack`
- `vel_to_filter`
- `filter_drive`
- `filter_tone`
- `filter_serial`

### Amplifier Section

Requirements:

- amp envelope
- velocity-to-amp
- final output gain
- optional pan/spread control
- no DC offset
- no uncontrolled level jump between short and sustained envelopes

Recommended controls:

- `amp_attack_ms`
- `amp_decay_ms`
- `amp_sustain`
- `amp_release_ms`
- `amp_vel_amt`
- `pan`
- `stereo_spread`
- `gain`

### Envelopes

Required envelopes:

- amp ADSR
- filter ADSR

Envelope requirements:

- time ranges should be musical, not arbitrary
- minimum attack should avoid clicks where possible
- short plucks should be easy to dial
- long pads should be easy to dial
- decay and release should cover both percussion and sustained instruments

Recommended ranges:

- attack: `1..5000 ms`
- decay: `5..10000 ms`
- sustain: `0..1`
- release: `5..12000 ms`

If envelope curve controls are added, they must be real curve behavior, not
unused decorative parameters.

### LFO Section

At least one LFO is required. Two are preferred.

Each LFO should support:

- sine
- triangle
- saw or ramp
- square/pulse
- rate in Hz
- optional tempo-sync later
- depth through destination-specific parameters
- retrigger on note start
- phase offset
- fade-in or attack time

Required destinations:

- pitch/vibrato
- pulse width
- filter cutoff
- amp or pan tremolo

The default preset should not use obvious LFO motion unless the preset role
calls for it. Micro-motion matters more than dramatic wobble.

### Performance Behavior

The synth must support, or be designed to cleanly accept host support for:

- mono/poly voice mode
- glide
- legato behavior
- pitch bend range
- mod wheel to vibrato or brightness
- velocity response
- keyboard tracking

If the host does not yet expose a behavior, the instrument spec should still
name the intended parameter so the agent does not invent incompatible surfaces
later.

## Parameter Surface

The default surface should be compact enough that an agent can reason about it.

Required groups:

- Oscillator
- Mixer
- Filter
- Amp
- Envelope
- LFO
- Performance
- Output

Avoid exposing twenty subtle implementation parameters as primary controls.
Internal constants are acceptable when they make the instrument sound better
and reduce useless parameter search space.

Every public parameter must be one of:

- directly audible
- useful for performance response
- useful for preset variation
- needed for modulation

Do not expose parameters only because they were convenient while implementing
the DSP.

## Modulation Contract

Use the existing DGenLisp modulation declaration system.

Required modulatable destinations:

- cutoff
- resonance
- filter_env_amt
- filter_drive
- pulse_width
- osc_detune_cents
- osc1_semitones
- osc2_semitones
- amp level or output gain, if safe
- pan or spread, if present

Preferred modulation sources:

- host LFO 1
- host LFO 2
- filter envelope
- amp envelope or auxiliary envelope
- velocity
- mod wheel / macro bus

Modulation depth ranges must be destination-specific. Do not use broad generic
depth ranges that allow nonsensical patches.

Examples:

- cutoff additive depth: approximately `-8000..8000 Hz`
- pulse width additive depth: approximately `-0.35..0.35`
- oscillator semitone depth: approximately `-12..12 st`
- fine detune depth: approximately `-25..25 cents`
- drive depth: small additive range, not enough to guarantee clipping

## Preset Contract

The agent should generate presets against this architecture before generating
new DSP from scratch.

Each preset must declare:

- name
- role
- short intent
- base note range where it is expected to work
- whether it expects mono or poly play
- parameter values
- any important modulation assignments

Required preset roles:

### Bass

Expected traits:

- mono by default
- strong fundamental
- controlled sub
- fast amp attack
- short-to-medium release
- filter envelope movement
- low to medium cutoff
- small detune or octave spread
- no wide stereo at low frequencies

### Lead

Expected traits:

- mono or low voice count
- moderate glide option
- expressive velocity or mod wheel brightness
- moderate sustain
- audible but controlled detune
- vibrato available but not always on

### Pluck

Expected traits:

- fast attack
- short decay
- low sustain
- filter envelope creates attack brightness
- velocity meaningfully changes level or brightness

### Pad

Expected traits:

- poly
- slower attack
- longer release
- wider oscillator spread
- subtle LFO or drift
- cutoff high enough to breathe but not brittle
- output level lower than bass/lead patches

### Stab

Expected traits:

- fast attack
- medium decay
- short release
- bright transient
- controlled sustain
- works as chords without clipping

### Soft Key

Expected traits:

- clean attack
- moderate velocity sensitivity
- low-to-medium resonance
- filter and amp envelopes balanced for repeated notes
- no excessive drift

## Agent Generation Rules

When the user asks for a bread-and-butter synth:

1. Prefer this architecture.
2. Generate or edit presets first if the architecture already exists.
3. Only create new DSP when the requested sound cannot be represented by this
   architecture.
4. Keep public controls stable unless the requested feature requires a real
   design change.
5. Use existing DGenLisp docs and examples for syntax.
6. Run `instrument_probe` for generated or changed DSP.
7. Reject silent, clipping, unstable, or obviously mis-gain-staged output.

The agent must not paper over a weak instrument by adding arbitrary effects,
extreme modulation, hidden gain boosts, or hard-coded note-specific behavior.

## Probe And Audition Requirements

For each DSP change, run `instrument_probe` through the same host compile/load
path as the app.

Minimum signal checks:

- no compile errors
- no load/init errors
- peak above a small minimum
- RMS above a small minimum
- no clipping
- output decays or sustains according to the selected preset role

Recommended probe set:

- low bass note
- mid note
- high note
- short gate
- sustained gate
- low velocity
- high velocity
- at least one chord for poly presets, if supported by the probe path

Initial command shape:

```sh
cargo run --bin instrument_probe -- <instrument-name-or-path> \
  --frames 4096 \
  --min-peak 0.01 \
  --min-rms 0.001
```

For preset-specific validation, pass parameter overrides with `--param`.

## Quality Bar

A preset is acceptable only if:

- it has an identifiable musical role
- the attack is intentional
- brightness is controlled
- low end is controlled
- velocity response is meaningful when enabled
- modulation depth is musically bounded
- output level is reasonable without external gain correction
- it remains useful across at least one octave, preferably more

A preset is not acceptable if:

- it is just raw saw or pulse into a static filter
- it needs a very specific note to sound good
- it clips internally or at the output
- it becomes silent under ordinary velocity or note values
- it uses random modulation as a substitute for tone design
- it exposes decorative parameters that do not matter

## Implementation Notes For DGenLisp

The following DGenLisp building blocks are expected to be sufficient:

- `phasor`
- `polyblep_saw`
- `polyblep_pulse`
- `triangle`
- `noise`
- `adsr`
- `biquad`
- `tanh`
- `clip`
- `make-history`
- `read-history`
- `write-history`
- `mod` destination access

If the implementation needs a missing primitive, add the primitive or host
support cleanly. Do not fake it with note-specific conditionals, hidden
hard-coded gains, or preset-specific DSP branches.

## V1 Deliverable

The first implementation should produce:

- `analog-bread-and-butter/dsp.lisp`
- `analog-bread-and-butter/ui.lisp`
- `analog-bread-and-butter.presets`
- at least six presets covering the required roles
- passing `instrument_probe` checks for the default preset and at least one
  bass, pluck, and pad preset

The implementation should favor a smaller excellent instrument over a large
surface with weak presets.
