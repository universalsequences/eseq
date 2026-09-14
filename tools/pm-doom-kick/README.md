# DOOM Kick resonant model study

One sample-free instrument with five fitted presets, installed in the local user
library as `Studies/DOOM Kick`. Play MIDI note 57 (A3) at velocity 1 to reproduce
the fitted register and level; other notes transpose it. The default is Kick 52.
Controls latch on trigger. Each trigger starts a one-shot; note-off does not cut
the recording envelope.

This is a useful reduced resonant approximation, **not a demonstrated perfect
match or an identified mechanical drum model**. Three damped resonances share
two pitch-relaxation time constants, with individual bend depth, onset, phase
and decay. A saturating recording stage and finite fade reproduce some of the
processing in the references. There are 26 scalar controls, no reference PCM,
residual playback, waveform tables, or sample-specific DSP branches.

The free resonances and empirical pitch/envelope law do not establish drum
geometry, membrane tension, or the original signal chain. Some fitted values
reach their allowed bounds. Remaining attack and tail errors are evidence that
this topology has limits, not a reason to label its controls as measured drum
physics. Generalization to other kicks has not been established.

## Tweaking the instrument

The custom synth panel exposes all 26 controls across Sweep, Body, Mode 2,
Mode 3, and Output pages. Eight main knobs remain visible around the page.
Click a section or a page button, then drag or enter values in the detail
controls. The normal host parameter-lock behavior is preserved. All changes
apply on the next hit. Saturation, knee shape, output level, and the final
hold/fade are on Output.

The dedicated `doom_kick_surface_controls_and_pages` nextest verifies all five
pages have visible, finite controls bound to all 26 manifest parameters, tests
normal edits and parameter locks, and exercises page navigation and base note.
The real `metal_seq capture` fixture is `tools/pm-doom-kick/capture.lisp`; its
1800 x 600 capture was visually inspected. The installed instrument also passes
`instrument_probe` after adding the UI.

## Selection and fidelity

The SQLite library was opened read-only. Exactly 43 samples had both `kick` and
`MF DOOM` tags. These are user tags, not independently verified provenance.
Candidates were compared using level-normalized time-frequency energy and RMS
envelopes. Ten candidates from the five closest neighborhoods were fitted;
the five best fits were retained. This is selection on fitting error, not a
held-out evaluation. All ten results remain in `fit-results.json`.

Compiled measurements at 16 kHz, at the original level and onset:

| Reference | Waveform NRMSE | 5 ms envelope NRMSE | Correlation |
| --- | ---: | ---: | ---: |
| 3 Kick -roundhouse | 0.2135 | 0.1278 | 0.9770 |
| Boom-Bap Kick 52 | 0.1126 | 0.0742 | 0.9936 |
| Boom-Bap Kick 60 | 0.2352 | 0.1579 | 0.9720 |
| Boom-Bap Kick 65 | 0.2306 | 0.1552 | 0.9731 |
| Boom-Bap Kick 72 | 0.2358 | 0.1365 | 0.9718 |

Lower NRMSE is better; correlation is not a percentage of perceptual similarity.
These measurements characterize a mono, downsampled comparison, not full-band
identity with the source files. The selected stereo sources are dual mono.
No listening test establishing perceptual equivalence has been performed.

Listen to `.local/pm-doom-kick/five-kicks-reference-then-model.wav`: each row
above appears in order, reference first and model second, without independent
gain normalization. `compiled-comparison.png` in the same directory overlays
waveforms and envelopes. These audio artifacts stay local.

## Reproduction

Requires the existing local sample database and referenced sample hashes, plus
NumPy, SciPy, SoundFile and Matplotlib. Run from the repository root:

```sh
.local/venvs/physical-models/bin/python tools/pm-doom-kick/analyze.py
.local/venvs/physical-models/bin/python tools/pm-doom-kick/fit.py
.local/venvs/physical-models/bin/python tools/pm-doom-kick/build.py --install
.local/venvs/physical-models/bin/python tools/pm-doom-kick/verify.py
```

Fitting uses deterministic multistart least squares at 8 kHz and recorded scalar
warm starts in `fit-seeds.json`; it can take several minutes. Reports and local
audio are regenerated. `build.py --install` replaces this study's local source
and preset bank. Omit `--install` to export only into this directory.

The verifier uses the pinned DGenLisp compiler and toolchain. It audits fusion,
compares all five presets with the independent equations at 16/44.1/48/96 kHz,
checks block partition equivalence including retriggers, silence, finite output
and state, all 26 controls, and combined parameter extremes at three pitches.
Maximum compiled/equation error was 0.0000163; block partition difference was
zero. These validate implementation, not sonic equivalence. All five installed
presets also passed the production host compile/load path via `instrument_probe`;
results are in `host-probes.json`.

```sh
target/debug/instrument_probe 'user:Studies/DOOM Kick' \
  --preset 'Boom-Bap Kick 52' --midi-note 57 --frames 24000 \
  --sample-rate 48000 --min-peak 0.01 --min-rms 0.001 --json
```

Remaining fidelity work is tracked in `eseq-l09f`. The important next modeling
question is how to capture the non-exponential attack/decay and residual pitch
motion without storing reference-specific trajectories.

## Default register

The intrinsic note mapping is now one octave higher. Base note 0 gives the
previous +12 sound. Existing project transpose settings are not modified;
return a manually raised Base note from +12 to 0 to retain that pitch. The
original fitted sample register is now A3 / MIDI 57. Reference-comparison
renders use that note; host probes also check all presets at the new default.
This change fixes the starting register, not the low-register tonal weakness,
which remains tracked in `eseq-vrlh`.

## Fast retriggers

Both studies now use the 808 Kick's two-slot exponential crossfade. Each hit
slot owns its clock, latched controls and DSP state. A new trigger starts the
other slot while the outgoing one decays through a 5 ms crossfade (-60 dB at
5 ms). The first hit bypasses the ramp. Trigger and gate rising edges are
recognized; a held-high trigger no longer continually restarts the sound.

`tools/instrument-support/kick-retrigger.lisp` is embedded by each builder.
`check_kick_retrigger.py` compares repeated output against independently
rendered isolated hits and calculated crossfade weights across all seven
presets at 5, 11, 31.25 and 62.5 ms intervals. It checks trigger continuity,
held triggers, block partition independence, and finite output with changing
pitch and velocity. The existing four-rate equation checks still pass.
At rates faster than the tested 5 ms interval, a reused slot can retain
appreciable gain; this is the same finite two-slot tradeoff as the 808 method,
not a guarantee for audio-rate retrigger modulation.

The local `fast-retrigger-before-after.wav` alternates direct-reset and
crossfaded sequences at 31.25 ms intervals, one pair per preset. Air is disabled
in this diagnostic to isolate the resonant discontinuities. All installed
presets also passed the host probe. No UI controls or preset values changed.

Two active slots cost about 1.9 times the previous DSP time in a warm 48 kHz,
128-frame local benchmark (including Python call overhead): about 9.5 us for
DOOM Kick and 19.7 us for Break Kick 53. These are small isolated measurements,
not a full-project realtime performance guarantee.
