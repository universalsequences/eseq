# PM Electric Bass

Load **Factory → Physical Models → PM Electric Bass**. Start with **Soft Pocket**
(the default) for rounded fingerstyle notes. **Felt Mute** is shorter and darker;
**Open Flats** rings longer; **Bridge Pick** emphasizes a narrow, bright attack;
**Roundwound** exposes more upper partials. The names describe voicings, not
calibrated replicas of particular strings or instruments.

The model produces a dry mono pickup signal on both output channels. Use the
host's normal mono/poly voice selection, note length and effects. The supplied
original 88 BPM phrase can be auditioned in `.local/pm-electric-bass/*.wav` after
running the verification script. No recording or copyrighted musical phrase is
embedded in the instrument.

## Controls

- **Pluck position** runs from the bridge toward the string center, as a fraction
  of the vibrating length. **Softness** widens the finger's contact footprint.
  **Velocity > brightness** makes quiet notes softer as well as quieter. These
  three controls are captured at note onset.
- **Open decay** specifies nominal fundamental T60 in seconds. **Damping** makes
  higher modes decay faster. **Mute** adds loss throughout the string, with a
  modest increase for higher modes. It approximates muting through modal losses;
  it does not simulate a hand's moving contact boundary.
- **Stiffness** stretches the partial spacing. **Tune** shifts the entire string.
  **Note-off decay** adds damping when the gate falls, without resetting state.
- **Pickup position** selects the spatial nodes heard by the pickup. **Width**
  averages velocity over a rectangular sensing aperture. Positions follow the
  vibrating length; there is no automatic open-string/fret assignment.
- **Tone** and **resonance** control a resonant two-pole lowpass approximation to
  pickup/electronics coloration. **Output** is a linear gain. There is no hidden
  distortion, limiter, cabinet or bass-enhancement oscillator.

## Model and assumptions

A finite modal expansion of a plucked, pinned stiff string uses 64 independent
contractive complex rotations. The initial displacement is triangular, smoothed
by a Gaussian contact footprint; the initial quadrature velocity is zero.
For mode number n, pluck fraction p, pickup fraction d and sensing width a:

```
B = 0.002 * stiffness²
f_n = f_1 * n * sqrt((1 + B*n²)/(1 + B))
A_n = 2*sin(pi*n*p)/(pi²*n²*p*(1-p))
contact_n = exp(-0.5*(pi*n*contact_width)²)
pickup_n = sin(pi*n*d) * sinc(n*a/2)
```

The velocity readout weights displacement by `f_n/f_1`, retaining signed spatial
cancellations. This uses the weak-damping quadrature approximation, not the full
derivative of the decaying displacement envelope. The inverse fundamental
normalization is a musical level calibration across notes, rather than absolute
volts from fixed physical displacement. Frequencies are normalized so changing
stiffness leaves the fundamental tuned. Rotations use radius `exp(-rate/sr)`:

```
rate_n = ln(1000)/decay_s
       + (0.2 + 24*damping²)*(f_n/1000)²
       + 35*mute²*(1 + 0.15*n)
       + (gate_is_off ? ln(1000)/release_s : 0)
```

Every term is nonnegative. Excitation fades between 0.40 and 0.48 times sample
rate; rotation frequencies are capped below Nyquist. The supported fundamental
range is 25–1200 Hz. Modal truncation limits the available brightness at low
notes; this is deliberately a bass-register instrument. There is no second
polarization, coupled string/body feedback, nonlinear fret collision, slap
contact or reference-recording fit. A retrigger replaces the modal state with a
new pluck; it does not model a finger catching an already vibrating string.

The modal derivation follows the initial-displacement formulation in Julius O.
Smith's [The Ideal Plucked String](https://www.dsprelated.com/freebooks/pasp/Ideal_Plucked_String.html).
The separate pluck/pickup spatial factors are also described by Mohamad, Harte
and Dixon in [Pickup Position and Plucking Point Estimation on an Electric
Guitar](https://webspace.eecs.qmul.ac.uk/s.e.dixon/pub/2017/MohamadHarteDixon-ICASSP2017.pdf).
The numerical loss laws and voicings here are original design choices, not
measured bass parameters.

Coefficient work runs every 16 samples and immediately on onset/note-off.
The control inputs enter `event-hold` before the modal expressions; placing it
only around a finished coefficient leaves the expensive upstream math running
every sample. Final latches keep the resonators running at audio rate.

## Steveland fit (vinyl bass reference)

`fit_steveland.py` fits the model to the two library samples "Bass Steveland 1"
and "Bass Steveland 2" (C2, 65.5 Hz, 44.1 kHz, 0.28 s and 0.31 s, the second
hand-muted at ~160 ms). Measured on the samples and now expressible in the
model:

- Only partials 1–4 matter; everything above 300 Hz is 35–50 dB down, and the
  roll-off is steeper than one pole pair. `output.steep` blends in a second
  identical SVF pole pair.
- The octave partial is full from the first cycle while the fundamental sits
  ~12 dB low and steps up to full level at about 40–50 ms. `pluck.attack_ms`
  drives a late smooth step on partial 1 only (0.3 → 1 between 55 % and 100 %
  of the time).
- Partial decay grows with ln(n), not f²: about −7, −36, −50, −60 dB/s for
  partials 1–4. `string.friction` adds `friction · ln(n)` to the loss rate.
- The recording floor is flat per octave from 1 kHz up, about 58 dB below the
  40–80 Hz band. `vinyl.hiss` (broad bandpass noise) and `vinyl.rumble` are
  faded with the gate so idle voices stay silent.

The fit is Nelder-Mead over twelve parameters on per-partial dB trajectories
(h1–h6, 10 ms steps) plus octave bands; results and the parameter set are in
`fit-steveland.json` and ship as the **Steveland Vinyl** preset (also the
basis of **Wobble Loop**). Partials 1, 2 and 4 track the recording within
about 1 dB over the note; partial 3 on the recording beats at ~25 Hz with a
second component the model does not have. The fit is on one pitch; other
notes follow the string's physics. `.local/pm-electric-bass/steveland-ab.wav`
plays sample 1, model, sample 2, model muted, then a 2.4 s model note.

## Tempo stretch (texture group)

The texture section imitates what an SP-303-class sampler does when a one-note
bass sample is played back at a lower tempo without changing pitch: the sample
is cut into fixed slices and each output slice re-reads the source from a
position that has fallen behind wall-clock time. `verify_stretch.py` checks the
compiled instrument against an independent NumPy model of this read pattern.

The string runs unchanged at its authored pitch into a delay line. The read tap
is a staircase: constant inside a slice of `grain_ms`, then increased by
`grain_ms * slow%` at each slice boundary, which is exactly the backward jump
of a slice-repeat stretch with tempo ratio `1 / (1 - slow/100)`. Slice
boundaries run on a free clock, so they land at arbitrary points in a note, as
they do when a sampled loop is stretched; the tap resets to zero at note onset,
so note timing is unaffected. The two taps at a boundary are crossfaded with a
raised cosine over `xfade_ms` (default 8 ms; a linear 2.5 ms fade was heard as
clicks on the low fundamentals). The crossfade must still be short: crossfading two
copies of the same waveform at different phases sweeps the instantaneous phase,
and over a long window that is a pitch bend of several semitones on a bass
note. Over a few milliseconds the same phase jump is heard as a level dip and a
faint click. The dip depth depends on the phase advance `2*pi*f*step`, so it
varies with note and slow amount; that is the note-dependent "wobble" of the
reference sound rather than a designed tremolo. With `slow` at 0 the tap stays
at zero and the output matches the dry string within float rounding. Changing `slow`
while a note rings affects only later steps; the tap never jumps mid-slice.

The tap is capped at 95,000 samples (about 2 s at 48 kHz); a very long held
note at a high slow setting eventually stops stepping and plays straight.

Because the tap lags wall clock by `slow%` of the note length, a note-off that
only damped the string would be heard late. The note-off decay
(`string.release_s`) is therefore applied a second time after the stretch, in
wall-clock time, so a release is immediate at any slow setting. At slow 0 this
makes the note-off decay roughly twice as fast as the string alone; the vinyl
presets use 0.1 s.
`latch(event-hold(...), tick)` explicitly returns those tensors to audio rate,
following the existing physical-model compiler contract. Modulation may wait up
to 15 samples. Rotations remain contractive as coefficients change, but abrupt
pickup/tone/volume parameter jumps can be audible; no transparent-automation
claim is made. The unplayed pluck-position latch is bounded before coefficient
division so cold silent voices stay finite.

## Verification

Install the pinned compiler and toolchain through the normal fetch scripts.
Python needs NumPy only. Run from the repository root:

```sh
python3 tools/pm-electric-bass/verify.py
cargo nextest run -p sequencer --bin metal_seq \
  -E 'test(=state_values::tests::pm_woodwind_ui_tests::electric_bass_surface_controls_and_pages)'
target/debug/instrument_probe 'factory:Physical Models/PM Electric Bass' \
  --midi-note 28 --sample-rate 48000 --frames 48000 --gate-frames 24000 \
  --min-peak 0.01 --min-rms 0.001 --json
```

`validation.json` identifies the exact DSP/compiler hashes and measured results.
Checks cover all presets, B0–E4 at 44.1/48/96 kHz, every control's extrema and
individual effect on audio, simultaneous extremes, automation, release,
velocity, retriggers, silent initialization, gate-only triggering, identical
stereo channels, and 32/128/256-frame partition agreement with off-grid notes.
An independent closed-form modal solution checks the compiled string before the
tone filter. Generated C is audited for scalar-history/tensor-loop fusion.
These are numerical checks, not listening-test or perceptual similarity scores.

The four-page panel uses the shared teal physical-model surface. Its focused
layout test checks every parameter, nonzero visible geometry, reactive bindings
and callbacks. Capture it through the production project path on macOS:

```sh
target/debug/metal_seq capture \
  --script crates/sequencer/ui/capture-fixtures/pm-electric-bass.lisp \
  --buffer fx --track 0 --width 1800 --height 600 \
  --out /tmp/pm-electric-bass-panel.png
```

## Stretch texture

**Slow %** lives beside Tone in the Output block. Start around **12–25**, or
load **Wobble Loop**. Zero passes the clean string. **Slice ms** and **Slice
crossfade ms** are on the Output detail page. Wobble rate is one per slice, so
tune Slice ms by ear against the record you are chasing; the crossfade should
stay in the low milliseconds. The algorithm is described under "Tempo stretch"
above. It is an original imitation of slice-repeat sampler stretching, not a
reverse-engineered SP-303/404.

Roland identifies the SP-303 as a major instrument used for Madvillainy in its
[sampler history](https://articles.roland.com/a-history-of-roland-samplers/).
The [SP-303 manual, page 42](https://cdn.roland.com/assets/media/pdf/SP-303_OM.pdf)
and [SP-404 manual, page 33](https://cdn.roland.com/assets/media/pdf/SP-404_OM.pdf)
describe pitch-independent Time Modify and acknowledge audible tremor/noise.
Neither manual specifies the algorithm. No claim is made that this effect
identifies the processing used on any particular album track. This **does not
lengthen your pattern or implement sample BPM sync**.

Run `python3 tools/pm-electric-bass/verify_stretch.py`. It renders the dry
string, applies the staircase read pattern in NumPy, and requires the compiled
instrument to match at three sample rates; it also checks process partitions,
silence, retriggers, release, bypass at Slow 0, live slow changes
never moving the tap mid-slice, and measures level-modulation depth and rate at
several notes. It writes `stretch-comparison.wav` under `.local/pm-electric-bass`:
clean, 12 %, then 25 %, with the same phrase timing and gain. `--baseline PATH`
optionally compares a saved earlier DSP source at Slow 0. Results, hashes and
per-voice native call timings are in `stretch-validation.json`. Timings include
the Python FFI call and are microbenchmarks, not full-app CPU measurements. No
hardware similarity or listening validation is claimed.
