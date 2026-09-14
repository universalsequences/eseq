# Break Kick 53

A playable, sample-free resonant approximation of the local `Boom-Bap Kick
53.wav`, installed as **Studies / Break Kick 53**. Play **A3 / MIDI 57** at full
velocity for the fitted register and level. The four-page custom UI exposes
all 14 controls; values latch on the next hit. Load its Boom-Bap Kick 53 preset
to restore the reference settings.

The reference hash is
`18b0fd84278df8a7bab6def01af9d61b170c511b3718c6206ce0cd45dd1ac063`.
The library database was queried read-only. The file is 44.1 kHz stereo,
approximately 684 ms long. Fitting and comparison use its mono downmix; source
provenance and the original recording setup have not been identified.

## What is modeled

Eight damped resonances, each with a rising amplitude envelope and exponential
frequency settling, are summed and clipped. Low resonances form the body;
higher ones provide the knock. A separate filtered noise burst adds adjustable
attack texture. The noise defaults (0.018 gain, 22 ms decay, 3.5 kHz
lowpass above a 700 Hz highpass) are sound-design settings, not recovered
recording parameters. A final 20 ms fade ends the one-shot at Length.

This is a **reduced resonant/spectral model**, not an identified physical model
of coupled drumheads, air cavity, beater contact or microphone placement.
Fitted modal frequencies and readout weights alone cannot establish that
mechanical system. No samples, residual waveforms or time-varying reference
trajectories are stored in the instrument. Some fitted relaxation/rise times
reach their bounds; those empirical shapes remain a limitation. The model is
not a general acoustic kick drum simulator and has not passed a perceptual
identity test.

The distinction follows the treatment of normal modes versus fitted resonant
models in [Smith, Modal Expansion](https://www.dsprelated.com/freebooks/pasp/Modal_Expansion.html).
The separate synthesized-noise approach is related to
[Excitation Synthesis](https://dsprelated.com/freebooks/pasp/Excitation_Synthesis.html);
this implementation is not a reconstruction of the original excitation.

## Results

The original three-mode DOOM Kick topology fitted to this reference achieved
waveform NRMSE 0.570 and 5 ms RMS-envelope NRMSE 0.480. `baseline.json` retains
that original-bounds result. `baseline-long-tail.json` tests the same topology
with extended hold/fade bounds, to avoid attributing an improvement solely to
its originally short gate. Extending those bounds gives waveform NRMSE 0.566
and envelope NRMSE 0.468, so that change alone does not close the gap.

The eight-mode fit achieves waveform NRMSE 0.277 and envelope NRMSE 0.195 at
8 kHz. The compiled model, including noise and final fade, measures approximately
0.279 waveform NRMSE, 0.195 envelope NRMSE, 0.962 correlation and -0.25 dB energy
level difference at 16 kHz. Lower error is better. Correlation is not a
percentage of perceptual similarity, and these are training-reference metrics,
not held-out validation or full-band identity measurements. Initial transients
and fluctuations in the tail remain visibly different.

Listen to `.local/pm-break-kick/reference-then-model.wav`: reference first,
model second, without independent gain normalization. The waveform/envelope
comparison is `.local/pm-break-kick/comparison.png`.

## Controls

- **Tune / Decay:** multiply the modal frequencies and decay time constants.
- **Body / Knock:** balance lower and upper resonances.
- **Attack / Pitch motion:** scale mode rise times and initial frequency bends.
  Negative bends are constrained to keep instantaneous frequency positive.
- **Air / Air ms / Air Hz:** noise amount, decay and lowpass cutoff.
- **Drive:** 1 is clean; higher values saturate the upper branch. The added
  distortion is highpassed at a pitch-tracking 180 Hz crossover to preserve the
  low body. Air is mixed afterward and is unaffected by drive or recording clip.
- **Recording clip mix / Clip ceiling / Output:** blend of the original
  hard-clipped recording character, clipping limit, and final gain.
- **Length:** one-shot duration; note-off does not cut the sound.

## Reproduce and validate

Run from the repository root using the existing physical-models Python venv
(NumPy, SciPy, SoundFile, Matplotlib):

```sh
OPENBLAS_NUM_THREADS=1 .local/venvs/physical-models/bin/python tools/pm-break-kick/model_fit.py
OPENBLAS_NUM_THREADS=1 .local/venvs/physical-models/bin/python tools/pm-break-kick/baseline.py
OPENBLAS_NUM_THREADS=1 .local/venvs/physical-models/bin/python tools/pm-break-kick/baseline.py --original-bounds
.local/venvs/physical-models/bin/python tools/pm-break-kick/build.py --install
.local/venvs/physical-models/bin/python tools/pm-break-kick/verify.py
```

The fitter initializes eight modes by greedy residual fitting with projected
linear readout weights, then jointly fits all modes through the clipping stage.
Decay, frequency, bend and attack are bounded. `build.py --install` replaces
this study's local source and UI, and adds missing presets while retaining
existing user presets; omit the flag to export only.
The research scripts reuse sample reading and metric functions from
`tools/pm-doom-kick`. Audio remains in the ignored local directory.

Compiled/noiseless equation comparisons pass at 16/44.1/48/96 kHz with maximum
absolute error below 0.00001. Block partition/retrigger output matches exactly;
all controls respond; output and floating DSP state remain finite at combined
parameter extremes and three pitches. The compiler fusion audit passes.
The noise generator stores uint32 RNG bits in the shared memory arena: the
verifier discovers those typed cells from generated C and does not interpret
them as floating-point samples. `instrument_probe` can falsely report NaN
state for such integer cells on some durations; follow-up `eseq-8lkc` tracks
proper typed diagnostics. This does not represent non-finite audio.

The production `instrument_probe` passed at 48 kHz for one second. The exact
`state_values::tests::pm_woodwind_ui_tests::break_kick_53_surface_controls_and_pages`
nextest passed: visible geometry, every manifest parameter reachable, normal
edits, parameter locks, page navigation and base-note control. The real Metal
capture from `capture.lisp` was inspected. No DSP engine changes were made.

## Listening revision

`Air` (previously `Air +12`) is a second preset based on the user's listening feedback: base note
0 with the new intrinsic octave shift, Air 2.5, recording clip mix 0.25, Drive 1, Output 0.4. It is a brighter
starting point, not a new fit or a replacement physical model. Existing saved
presets are retained. Lower clipping restores peaks, so the preset uses lower
output gain for headroom.

`check_drive.py` renders Drive 1/3/5 at +12 in
`.local/pm-break-kick/drive-1-3-5-air-plus12.wav`. It checks that the added Air
signal is unaffected by drive and that 25–100 Hz energy changes by less than
1 dB. Measured changes were below 0.02 dB. This validates separation of the
paths; whether the new distortion sounds better remains a listening judgment.

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
