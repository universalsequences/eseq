# PM Ride Kit — modal ride identified from the "08. Forza G" sample

One sample-free factory instrument, shipped as **Drums / PM Ride Kit** (browser
category *Hats & Cymbals*). It re-creates the stick strokes of the library
sample `08. Forza G (Quella Donna) [Forz…` tagged drums/percussion (10.09 s,
32.5 kHz stereo; `.local/samples/2a9a7dbb…392d.wav`, verified by SHA256 in
`analysis.json`). The library has a second, 20 s file with the same title
(bass, whispers); this is the 10 s one. The ride is panned hard left and a
whispered voice sits in the right channel (L/R correlation 0.04), so only the
left channel is analysed. The reference is commercial material: the
instrument and these tools store fitted coefficients only (no PCM, recorded
frames or envelopes).

The existing **PM Ride** (`tools/pm-cymbals`) is a different, generic plate
model calibrated from another pack; this one is a measured stroke kit like
PM Bongos and PM Tabla.

## Playing it

Twelve strokes repeat in **every octave**, one per note name, as in PM
Bongos/Tabla. The C4 octave plays them at the recorded pitch; each octave up
or down transposes the whole cymbal (±3 octaves, then it clamps). Velocity 1
at Hardness 0.5 reproduces the recorded level.

| Note | Stroke | Reference onset (s) |
| --- | --- | ---: |
| C | Ride | 9.156 |
| C# | Ride Push | 3.039 |
| D | Ride Soft | 6.583 |
| D# | Ghost | 3.650 |
| E | Tip | 7.215 |
| F | Dark | 8.829 |
| F# | Shoulder | 6.228 |
| G | Shoulder Accent | 9.816 |
| G# | Shoulder Soft | 3.977 |
| A | Ping | 7.572 |
| A# | Bell | 5.239 |
| B | Bell Open | 4.617 |

Names are listening descriptions from each stroke's modal spectrum (dark bow
strokes carry their energy at 175–700 Hz; "shoulder" strokes 15–20 dB less
there and more at 5.6–11 kHz; the two bell strokes are dominated by one
partial near 3.4 kHz), not player annotations. The sample holds 22 strokes;
the other 10 are played in comparisons by the key with the closest spectrum.

**One cymbal.** `instrument.json` binds the host's voice controls, and the
default Voicing is 1 (mono, retrigger): every hit is injected into the same
ringing plate, as on the record, instead of starting a fresh voice. A
retrigger never clears DGen voice state, so the ring carries on underneath.
Voicing 0 (poly) gives every hit its own plate; the voice count is the host's.

Controls (14 on the teal surface; main knobs Hardness, Click, Decay, Choke,
Width, Tune, Tone, Output):

- **Hardness / Vel > timbre:** stick contact time; a harder stick
  redistributes a fixed modal energy budget toward upper modes.
- **Click / Click tone:** the stick's contact noise.
- **Decay / Muffle / Choke:** natural ring, tape on the plate (loss grows
  with frequency), and a hand that grabs the cymbal when the key is released.
- **Wash:** level of the dense wash layer (below).
- **Width:** scatters the modes across the stereo field with a fixed per-mode
  pan and decorrelates the wash; 0 is the recorded mono ride.
- **Tune / Drive / Tone / Output.**

Presets: Recorded, Wide Ride, Taped, Dark Stick, Bright Stick, Grab, Dusty
Record.

## Model

One mono voice holds the whole plate:

- **640 ring modes + 48 attack modes,** all shared by every stroke (a cymbal's
  modes belong to the plate, not the stroke); a key only selects the complex
  residues its stroke injects. Recomputed coefficients run on a 16-sample
  event clock.
- **Wash:** ~1500 weaker identified modes are not run individually. What
  the modes leave of the record's power is carried by white noise through a
  third-octave bank (20 bands, each two cascaded TPT state-variable
  band-passes), per band a fast and a slow power component per stroke with a
  measured build-up time. Without it the 640-mode tail is a sparse line
  spectrum (spectral flatness −25 to −46 dB against the record's −13 dB) and
  reads as chimes, not wash.
- **Click:** a band-pass noise burst per stroke at the residual click's
  measured centre, level and decay.

## Identification (`analyze.py`, `modal_fit.py`)

1. High-pass at 150 Hz (the ride's lowest mode is ~180 Hz; the song bleeds
   50 Hz-series bass and hum into the channel).
2. **Strokes** are 6–16 kHz stick bursts ≥ 20 dB above their surroundings
   (22 found).
3. **Ring poles: multi-segment ESPRIT** in 50 Hz complex subbands. Between
   strokes every band is a free sum of the same damped exponentials, so the
   Hankel matrices of all inter-stroke segments share one signal subspace:
   poles come directly, no nonlinear search (2135 poles, 0.3–25/s). Slow
   lines on the 50 Hz series are dropped as bleed.
4. **Residues** per (stroke, pole) by linear least squares over the whole
   sample, each band solving with every pole within 60 Hz and keeping its own
   core. Residues are rotated back from the band's demodulation frame
   (e^{i2π f_c t_s}); onsets are snapped to the sample grid (half a sample is
   90° at 16 kHz).
5. **Selection:** 640 poles, the budget split between third octaves ∝
   count^0.7 × energy^0.3. A pure energy ranking keeps 4 of ~720 poles above
   11 kHz. The kept poles' residues are then **re-solved without the
   others**: close pairs with cancelling residues otherwise leave a kept pole
   carrying its dropped partner's energy.
6. **Attack poles:** 48 fast poles (12–900/s) shared by all strokes, fitted by
   variable projection with an analytic (Kaufman) Jacobian on the first 80 ms
   left by the rings.
7. **Wash:** per third octave (rectangular, by FFT), the record's band power
   minus the modal model's, fitted as a hiss floor plus per-stroke fast and
   slow components with build-up, NNLS on relative error. The deficit is not
   clipped at zero (both powers beat; clipping biases it upward) and the
   residual's power is never used directly: where the modes have the right
   level but not the record's chaotic attack waveform, the residual holds both
   energies.
8. **Click:** what is left in 2.5–16 kHz in each stroke's first milliseconds.

The pre-stroke ring is 10–25 dB below each new stroke's energy, so the
measured residues are effectively fresh strokes: a key alone sounds like its
stroke, and the sequence of keys re-plays the record.

## Calibration (`compare.py --calibrate-noise`)

The click is scaled per key to its measured contact click. The wash bank's
analytic unit-variance normalization ignores skirt overlap and warping near
Nyquist; its band gains are solved together by NNLS through the bank's
digital response matrix against rectangular (FFT) band energies of a rendered
replay. Band-filter measurement would mix strong bands into the steep top
octave. Iterate `build.py` / `compare.py --calibrate-noise` until stable.

## Fidelity (`compare.py`, `comparison.json`)

Calibration equals test (same recording). One voice re-plays the whole sample
as retriggers at the recorded onsets, rendered at 48 kHz and resampled to
32.5 kHz; each stroke window is scored against the high-passed left channel.
Mean |level error| (dB) of the 12 installed strokes:

| 0–5 ms | 5–15 ms | 15–30 ms | 30–60 ms | 60–150 ms | 150–300 ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0.9 | 1.3 | 0.8 | 0.4 | 0.2 | 0.5 |

Known limits:

- The first ~15 ms are the least exact: a stick hit's opening milliseconds
  are near-chaotic plate motion and contact noise; band levels there are
  within about ±2 dB (the 11–16 kHz octave's opening click ~+5 dB).
- 175–700 Hz runs 2–3 dB low at 150–300 ms, and the top octave's late level
  is near the record's hiss floor, which the model does not reproduce.
- Texture: from 30 ms the tail's spectral flatness and line density match the
  record at 1.4–5.6 kHz (−15/−13 dB vs −15/−14); 5.6–11 kHz is ~2.5 dB more
  noise-like and the quiet top octave flatter still.
- Strokes substituted by another key are approximations (all-stroke error is
  ~0.5 dB higher).

Listening material (local, not committed) in `.local/pm-ride/`:
`sample-ab.wav` (record, then replay), `model.wav`, `strokes.wav` (the twelve
keys alone), `groove.wav` (a short groove at Width 0.5).

## Cost (`performance.py`, `performance.json`)

Native process CPU at 48 kHz with two strikes per second: ~10.3% of one core
for the **whole cymbal** (one voice), against 0.8% per voice for PM Tabla and
1.5% for the generic PM Ride. In Poly voicing each voice costs this much.

## Checks

- `verify.py` (`verification.json`) at 44.1/48/96 kHz: every key × preset with
  other strokes landing on the ring; every control at min/max and all-min /
  all-max corners; a 32-hit roll on one plate; finite audio and state, peak
  < 4; 128/512-frame blocks identical; key k+12n equals key k with Tune 12n
  and octaves beyond ±3 clamp; Choke inert while held, < −40 dB 300 ms after
  release; Width changes a key's power by < 3 dB (measured +1.7 dB at full width).
- `cargo nextest run -p sequencer -E 'test(pm_ride_kit_surface_controls_and_pages)'`
  checks all five pages, the eight main knobs and every control's parameter
  and p-lock edits, reading the tracked `model.lisp` / `ui.lisp`.

## Reproduction

Needs the sample in `.local/samples`, the physical-models venv and the
fetched DGenLisp compiler. The analysis takes ~2 minutes.

```sh
P=.local/venvs/physical-models/bin/python
cd tools/pm-ride
$P analyze.py
rm -f noise-calibration.json
for i in 1 2 3; do $P build.py && $P compare.py --calibrate-noise; done
$P build.py && $P build.py --check
$P compare.py && $P verify.py && $P performance.py
```

`engine.lisp.in` is the implementation. `build.py` generates the factory
`content/instruments/Drums/PM Ride Kit/` (`dsp.lisp`, `ui.lisp`,
`instrument.json`, presets, attribution) and the `model.lisp`, `ui.lisp`,
`instrument.json` and `model.presets` copies here that the surface test reads.
