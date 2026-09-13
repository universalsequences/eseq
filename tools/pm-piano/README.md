# PM Piano

One playable, physically informed piano across A0–C8, calibrated from all 30
Salamander notes in the factory sample package. The instrument generates its
sound from struck, damped string modes and a small soundboard model. It does
not play samples or embed recorded waveforms, phases, or noise.

The Physical Models surface has eight main knobs and eight pages. The
bank contains **Salamander Grand, Soft Felt, Bright Studio, Compact Wood,
Loose Strings, Dusk Tremolo, Prepared Wire, Reverse Dust, Blooming Felt,
and Reverse Shadow**. Every preset recalls all 37 parameters of the same instrument.

## Playing and shaping

| Control | Behavior |
| --- | --- |
| Decay | Scales the natural string decay while a key is held, and under the damper tail. |
| Release s | Adds key-up damping, from 0.02 to 20 seconds. It leaves the attack and held note unchanged. The actual tail also depends on natural string decay. |
| Pedal lift | Continuously lifts the dampers; at 1 a released string rings with its natural decay. This is an instrument parameter, not a new MIDI CC mapping. |
| Free treble | Optional undamped upper strings. Defaults to 0 so Release works across the entire keyboard. |
| Hardness / Contact | Felt brightness and contact duration. Lower velocity makes the strike softer as well as quieter. |
| Strike position | Moves the excitation point along the string, changing harmonic nodes. |
| Damping / Stiffness | Frequency-dependent losses and overtone dispersion. Large stiffness values move toward metallic, alien timbres. |
| Aftersound / Unison / Width | Slow string modes, beating, and stereo spread. Bass transitions toward a single string. |
| Register st | Borrows another keyboard region's modal voicing while keeping the played pitch. |
| Lid / Register balance | Spectral tilt and optional compensation for the factory recordings' quieter treble. |
| Wood color / Body size / Resonance | Soundboard coloration, nominal modal frequencies and strength. |
| Tuning stretch | 0 uses equal temperament; 1 follows the measured recording tuning. The highest reference C is about 90 cents sharp, so this is an audible choice. |
| Tremolo / Auto pan | Independent periodic amplitude and stereo movement. |
| Reverse blend | 0 keeps the normal struck piano; 1 gives only the reverse swell. Intermediate values layer both, and blend can change live. |
| Reverse seconds | Sets the rise to its crest, from 0.05 to 8 seconds. Trigger the note this far ahead of the intended landing. |
| Rise curve | Below 1 gives a broad, early build; above 1 concentrates the rise near the crest. |
| Ringing tail | 0 ends promptly after the crest; 1 continues with natural string decay. Intermediate settings shorten the ringing tail. |
| Drive / Tone / Output | Final saturation, filtering and level. |

Release describes the extra damper loss: a fundamental loses 60 dB in the
chosen time from the damper alone, with upper partials damping faster. Natural
string losses remain active, so turning Release up cannot sustain a string
that has already decayed. Pedal lift can also change during a tail. It does not
re-excite a silent string. The host currently permits custom voices to ring
for at most 20 seconds after key-up; voice stealing can end them sooner.

Hold the key through a swell, or lift Pedal to 1. Key-up still applies Release,
so a short gate with a short Release can stop a rise before its crest. Time,
curve and ringing-tail settings are sampled at each onset: edits take effect
on the next note, keeping the current landing time predictable. Blend stays
live. At 80 BPM, 0.75 seconds is one beat and 1.5 seconds is two beats.

**Reverse Dust** is a short reverse hit, **Blooming Felt** is a 2.4-second rise
that flows into a sustained felt piano, and **Reverse Shadow** layers the
normal strike with a later swell. Their full settings can be combined with
all the existing hammer, string, voicing, body and motion controls.

The eight display diagrams illustrate the mechanisms and follow their
parameter bindings. They are not oscilloscopes or measurements of live audio.

## Model

`analyze.py` estimates fundamental tuning, stiff-string dispersion, modal
amplitudes and two-stage energy decay from each reference. The string law is
`f(n) = f(1) n sqrt((1 + B n²) / (1 + B))`. Two positive decay components model
the initially fast tone and slower aftersound associated with coupled string
normal modes and polarizations. See Smith's [piano synthesis discussion](https://www.dsprelated.com/freebooks/pasp/Piano_Synthesis.html)
and [string coupling effects](https://dsprelated.com/freebooks/pasp/String_Coupling_Effects.html).

`calibrate.py` embeds only resonant coefficients: 30 register rows, interpolated
continuously, with 64 initial string modes and two detuned sets of 32 slow
modes. The analysis retains 96 modes so truncation can be assessed. Higher
modes carry negligible measured energy in this set. For the highest two
references, only one strong partial is available, so dispersion continues the
last identifiable treble trend instead of fitting recording noise. Base decay
rates have a positive floor of 0.22/s so noise-floor fitting cannot create an
endlessly ringing mode.

Each mode is a damped complex rotation. Positive losses keep an unforced mode
contractive even when its frequency changes. A three-pole filtered impulse
approximates felt contact; its reference response is removed from the fitted
residues before applying the live, velocity-dependent force. Strike position,
contact time and hardness therefore alter excitation without resetting the
ringing strings. This is a commuted, modal approximation rather than a full
nonlinear hammer collision solver. The decomposition and tradeoffs are
described in [Bank et al., 2003](https://home.mit.bme.hu/~bank/publist/jasp03.pdf)
and [commuted piano synthesis](https://www.dsprelated.com/freebooks/pasp/Commuted_Piano_Synthesis.html).

Event-held controls schedule coefficient math every 16 samples, then explicit
frame-rate latches hold the results for the audio-rate recurrence. Control latency is at most 15 samples
(0.34 ms at 44.1 kHz). Modes fade out between 0.40 and 0.47 times the sample
rate. Three broad, short soundboard modes and synthesized mechanism noise add
hammer/key movement; a body filter and output section provide further color.

### Reverse contour

The swell reuses the same modal frequencies, calibrated residues, positive
loss rates, hammer-contact magnitude, unison spread and output processing.
It is an additional synthesis contour, not recorded audio played backward
or feedback with negative damping. The ordinary struck-string bank remains
available continuously for live blending.

For note age `t`, length `L`, and rise curve `c`, let `p = clamp(t/L, 0, 1)`.
The envelope evaluates the modal decay at
`L * (1 - p^c) + max(0, t-L)`. Each modal magnitude is therefore bounded by
its strike magnitude, approaching that magnitude at the crest, then decaying
naturally. A `p^c` gain ensures that slowly decaying bass modes also rise
clearly. An 8 ms entrance ramp and 5 ms per-mode magnitude smoothing keep
retriggers and edits smooth. Each carrier is a normalized complex oscillator,
so long swells and changing tuning do not accumulate oscillator gain or phase
counter precision errors. Normalization uses one Newton correction around
unit squared magnitude: multiply both components by `1.5 - 0.5*(x*x+y*y)`.
The carrier starts at unit magnitude; paired sine/cosine rotations keep it in
the correction's stable interval. This avoids a square root and two divisions
per mode per sample. It applies only to the unit carriers, never to the struck
modes whose amplitudes must preserve their physical decay.
A whole-seconds plus integer-samples age clock
continues natural decay through very long held notes.

Key-up damping accumulates separately and irreversibly until the next onset;
lifting the pedal stops further damper loss but does not restore energy
already damped away. Ringing tail adds a positive loss after the crest:
`(1-tail)^2 * ln(1000)/0.035`. At 0 the added loss reaches -60 dB in 35 ms;
at 1 there is no added end loss. Envelope smoothing and modal interference
mean the measured waveform peak need not fall on the exact crest sample.

This reverses modal amplitude and spectral evolution, **not the exact phase
of a recorded piano**. It also does not reverse room/reverb tails or mechanism
noise. Blend fades the normal hammer/key noises out with the struck bank,
while the swell still passes through the shared soundboard and output color.
A long treble swell can reach full level because it is not fading up a note
that has already decayed.

The extra carriers run even at zero blend, preserving continuous phase for
live blending. All 256 struck and reverse modes remain active. The September
2026 follow-up measured about 115 microseconds per 128-frame call at 48 kHz,
roughly 25% less CPU than v0.1.21, using the same piano patch and native ABI.
This is a one-voice measurement, not a guaranteed voice count or a host-wide
benchmark. See [the measured results](../../docs/pm-piano-performance-followup-2026-09-12.md)
for whole-project measurements and sound comparisons.

The factory has **one velocity layer, v8**. Softer/harder dynamics are modeled,
not measured against additional Salamander velocity layers. The model does
not reproduce the recording's stereo microphone field, room, noise or exact
attack phase, and it does not simulate sympathetic resonance between separate
played notes. It aims for the set's register character and useful continuous
controls rather than a sample-identical reconstruction.

## References and license

Source: **Alexander Holm, Salamander Grand Piano V3**, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/).
Original collection: [Salamander Grand Piano V3](https://archive.org/details/SalamanderGrandPianoV3).
The factory's compact MP3 versions came through
`felixroos/dough-samples@9eacfc86ec4393e68a463ff52b01c19cfaa77f38`.
The derived calibration tables retain this attribution and CC BY 3.0 license.
The recordings remain in
`content/packages/universalsequences.factory-samples/samples/piano/`.
Per-file SHA256 hashes and measurements are recorded in `reference-analysis.json`.

## Reproduce and verify

Use Python with NumPy, SciPy and SoundFile, plus the repository's fetched
DGenLisp compiler and toolchain. Run from the repository root:

```sh
./scripts/fetch_dgenlisp.sh
./scripts/fetch_dgen_toolchain.sh
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/analyze.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/calibrate.py --check
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/compare.py
ESEQ_PM_VERIFY_DIR=/tmp/eseq-piano-roundtrip cargo nextest run -p eseqlisp --test pm_woodwinds -E 'test(=factory_piano_sidecar_preserves_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/verify.py --roundtrip '/tmp/eseq-piano-roundtrip/PM Piano/dsp.lisp'
# The pre-swell instrument is preserved by this repository commit.
git show '70618872:content/instruments/Physical Models/PM Piano/dsp.lisp' > /tmp/pm-piano-before-swell.lisp
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/verify_swell.py --baseline /tmp/pm-piano-before-swell.lisp --roundtrip '/tmp/eseq-piano-roundtrip/PM Piano/dsp.lisp'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/demo.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-piano/demo_swell.py
cargo run --bin instrument_probe -- 'Physical Models/PM Piano' --frames 96000 --gate-frames 12000 --param damper.release_s=5 --min-peak 0.01 --min-rms 0.001 --json
cargo nextest run -p sequencer --bin metal_seq -E 'test(/pm_woodwind_ui_tests::.*surface_controls_and_pages/)'
cargo run -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/pm-piano.lisp --buffer fx --track 0 --width 1800 --height 600 --out /tmp/pm-piano.png
```

`analyze.py` recomputes the measurements and may take several minutes. Omit
`--check` from `calibrate.py` to update the generated table section deliberately.
Graph layout and executable DSP must be kept in sync after DSP edits. The
sidecar regression exports actual patch-editor writeback; `verify.py` compiles
it and compares audio, including key-up and off-grid parameter changes.

`validation.json` records the source/compiler hashes and signal checks:
88 keys, complete preset recall, individual and combined control extremes,
automation, host modulation, velocity silence, three sample rates, independent
Release and full pedal lift, gate-only onset, irregular process calls, graph
save, generated-C fusion audit and one-voice CPU timing. Timing is an offline
measurement on the recorded platform, not a guaranteed polyphony count.

`reference-comparison.json` reports normalized string partial-band error and
actual RMS levels for all 30 references in five time windows. This spectral
metric does not measure attack phase, room or overall perceptual similarity.
Late high-register windows can compare the recording's noise floor after its
string tone has died. A/B clips retain relative reference/model levels, with
the reference first and the synthesis after half a second of silence.

Previews and eight production UI captures are generated under ignored
`output/`. `release-comparison.wav` plays the same note with 50 ms, 500 ms and
5 s Release, then full pedal lift. `groove-reference-grand.wav`,
`groove-soft-felt.wav` and `seven-voices.wav` contain only synthesized audio.
`demo_swell.py` adds `reverse-lengths.wav` (the same chord with 0.25, 1 and
3 second rises), `reverse-trip-hop.wav` (an 80 BPM piano phrase), and
`reverse-felt-bloom.wav` (a long rise flowing into a natural tail).
No per-clip peak normalization or limiter hides the measured levels.

`swell-validation.json` adds 112 renders, including timing at 44.1/48/96 kHz,
50 ms–8 s crests, two-minute held-note decay, zero-blend comparison against
the pre-swell instrument, Release/Pedal interaction, per-note timing and shape,
live blend, all four host modulation destinations, combined swell extremes,
retriggers, silent idle, short/irregular host calls, graph-save
comparison and CPU timing. Five production `instrument_probe` runs are in
`swell-host-validation.json`; preview levels are in `swell-demo-levels.json`.

Validation was performed on macOS ARM64. The compiler release includes the
streaming hop-clock and mixed-rate scalar-loop fixes needed by this graph;
Linux remains on its independent older pin and has not been validated here.

Recorded validation: **338 general signal checks**, 11 parser/numeric-writeback tests,
three factory sidecar tests, and all five physical-model UI layout/binding
tests passed in the original piano session. For the swell change, the piano
sidecar and all eight piano UI pages were revalidated with focused tests. Five production host probes are in `host-validation.json`.
The swell graph-save comparison differed by less than 0.000002 peak per sample;
zero blend differed from the pre-swell instrument by less than 0.000001. Across the
first three comparison windows (25 ms–1 s), the median normalized string-band
error was 0.41 dB and the maximum was 3.42 dB; this is the restricted spectral
metric described above, not a perceptual similarity score. The 32-mode slow
bank removes at most 0.105% of estimated total modal energy at 0.2 s.

The required macOS compiler is pinned to
[DGenLisp v0.1.17](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.17).
Its two fixes passed 29 and 41 focused compiler tests respectively. The
eseqlisp scientific-notation parser fix is also required when regenerating or
saving calibrated tensor graphs; it prevents tiny coefficients from being
split into a large mantissa and a separate symbol.

An exploratory diagnostic with 16 output channels crashed in the audition
path; that separate case is tracked as `dgen-xv0` and has not been attributed
to the compiler or harness yet. The instrument uses the validated stereo path.
