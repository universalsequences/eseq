# PM Ride release 2 — six measured rides, one shared plate each

**Physical Models / PM Ride** ships as release 2 of its lineage. Release 1
(the reduced-plate model from `tools/pm-cymbals`) is frozen under
`content/instruments/Physical Models/PM Ride/versions/1/` with its own preset
bank, so projects saved against it keep loading and sounding the same
(`docs/instrument-versioning-spec.md`). New tracks get release 2.

Release 2 applies what PM Ride Kit (`tools/pm-ride`) learned to the Donit
pack that release 1 was calibrated on.

## What changed and why

| Release 1 | Release 2 |
| --- | --- |
| 8 resolved modes + a delay network: dense body at arbitrary frequencies | 448 measured ring modes + 32 attack modes per cymbal, plus a measured wash |
| Character interpolates fitted parameters across 29 files | Character chooses one of six measured cymbals; never blends |
| Calibrated on 17 band powers | Calibrated on band levels **and** fine-spectrum texture (flatness, lines/kHz, spread) |
| No noise tail | Third-octave wash fitted to the record's unclipped power deficit, with build-up |
| Every hit a new voice | One-cymbal voicing: hits land on the same ringing plate |

The pack's 29 "ride" files are not one cymbal: their strongest partials agree
at chance level (5–15%, except Ride/Ride_2 at 32%), and centroids span
0.3–7.6 kHz. Averaging parameters across them describes no cymbal.

## The cymbals

Unclipped, single clean strike, ~2 s ring to −30 dB; clipped files
(Ride_2/4/6/7/9/25) and the 300 Hz thud (Ride_5) are excluded.

| Character | Name | Reference | Centroid |
| ---: | --- | --- | ---: |
| 0 | Dark | Ride_18 | 1.75 kHz |
| 1 | Warm | Ride_22 | 3.05 kHz |
| 2 | Classic (default) | Ride_11 | 3.6 kHz |
| 3 | Dry | Ride_12 | 5.2 kHz |
| 4 | Bright | Ride_10 | 6.4 kHz |
| 5 | Crisp | Ride | 7.2 kHz |

Cymbals are matched in energy over their first half second, so Character
does not jump in level; a fixed output trim keeps the hottest near 0.5 peak at
velocity 1 (no limiter; extreme settings need mixer headroom).

## Controls

Character, Bell (reweights the cymbal's 12 strongest partials; 1 = as struck),
Size (frequencies down and rings longer together — a geometric heuristic),
Tune, Tracking (0 = every key plays the cymbal; 1 = keys transpose from C4),
Hardness / Vel > timbre (energy-conserving stick contact), Click / Click tone,
Decay, Muffle, Choke (grab on key release), Wash, Width (per-mode stereo
scatter; the recordings are mono), Drive, Tone, Output, Voicing (1 = one
cymbal). Presets: Classic, Dark, Warm, Dry, Bright, Crisp, Bell, Taped, Grab.

## Identification (`analyze.py`)

Per cymbal, with PM Ride Kit's functions (`tools/pm-ride/analyze.py`,
`modal_fit.py`); see that README for the method. New for single long hits:

- **Two ESPRIT passes.** A single strike's subspace (order 20 per 50 Hz band)
  is filled by the modes that are loud early; the modes that rule the late
  ring are only found by a second pass from 0.37 s. Poles are merged (~5,600
  per cymbal).
- **Selection by tail energy.** Poles are ranked by their energy after
  0.15 s, the audible tail. Ranking by total energy left the 5.6–11 kHz ring
  10–13 dB low after 1 s.
- Tested and rejected: relative-error residue weighting (worse everywhere)
  and same-frequency "build-up" partner poles (no gain: the late energy lives
  in other modes, not in the early ones growing).
- Even all ~5,800 poles leave Dark's 1.4–5.6 kHz ring 2–4 dB low after
  0.3 s (energy keeps flowing into the plate's dense modes); the slow wash
  component carries it.

## Calibration (`compare.py --calibrate-noise`)

The click is scaled per cymbal. For the wash, one strike is too little data
for a free per-band deconvolution (it produced 279× gains): bank leakage is
undone analytically by a regularized NNLS through the bank's digital
response, pulled toward no correction, and the render corrects only
normalization, by at most 0.5–2× per band per pass, in bands with energy.
Iterate `build.py` / `compare.py --calibrate-noise` three times.

## Fidelity (`compare.py`, `comparison.json`)

Calibration equals test. One strike per Character (C4, velocity 1, Width 0),
rendered at 48 kHz, resampled to 44.1 kHz, scored against its high-passed
recording. Mean |level error| over the six cymbals (dB):

| 0–5 ms | 5–15 | 15–30 | 30–60 | 60–150 | 150–300 | 300–600 | 600–1000 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0.7 | 0.7 | 0.4 | 0.3 | 0.4 | 0.6 | 0.8 | 1.3 |

Spectral spread (FFT bins holding 90% of the first second's power), the
measure where release 1 had about half its reference's (384 vs 664):

| | Dark | Warm | Classic | Dry | Bright | Crisp |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Recording | 880 | 1089 | 3743 | 3937 | 1546 | 2424 |
| Model | 1021 | 914 | 3582 | 4437 | 1993 | 2556 |

Known limits: at 300 ms the tail is more noise-like than the recordings
(flatness −7 to −19 dB vs −12 to −30, fewer lines per kHz), most on Dark,
Warm and Dry, where the wash carries the late energy the modes cannot; Dark
runs 2–3 dB low after 0.3 s. Listening material (local, not committed):
`.local/pm-ride2/ab.wav` (each cymbal: recording, then model) and
`groove.wav` (eighths on every Character, Width 0.3).

## Cost (`performance.py`, `performance.json`)

~8.1% of one core at 48 kHz for the whole cymbal (one voice; coefficients
recompute on a 64-sample tick). Release 1 is 1.5% per voice, but in a ride
pattern several of its multi-second voices ring at once.

## Checks

- `verify.py` (`verification.json`) at 44.1/48/96 kHz: every Character ×
  preset with strikes landing on the ring; every control at min/max and the
  all-min/all-max corners; a 32-hit roll; finite and bounded (< 4); 128/512
  blocks identical; Tracking 0 makes every key identical, Tracking 1 equals
  Tune (within float32 pitch rounding); a Character change mid-ring leaves the
  ring untouched; Choke inert while held, < −40 dB 300 ms after release;
  Width changes power < 3 dB.
- `cargo nextest run -p sequencer -E 'test(ride_surface_controls_and_pages) | test(ride_release_1_surface_controls_and_pages)'`
  (release 2 and frozen release 1 panels), and
  `cargo nextest run -p eseqlisp --test pm_woodwinds -E 'test(factory_cymbal_sidecars_preserve_executable_controls)'`.

## Reproduction

```sh
P=.local/venvs/physical-models/bin/python
cd tools/pm-ride2
$P analyze.py                       # ~1.5 min; needs samples-to-analyze/
rm -f noise-calibration.json
for i in 1 2 3; do $P build.py && $P compare.py --calibrate-noise; done
$P build.py && $P build.py --check
$P compare.py && $P verify.py && $P performance.py
```

`build.py` writes release 2 at the top of the lineage folder
(`dsp.lisp`, `ui.lisp`, `instrument.json` with `current: 2`, ATTRIBUTION),
`Physical Models/PM Ride.presets`, and the copies here that the surface test
reads. It never touches `versions/1/`; `tools/pm-cymbals` now builds PM Ride
into that frozen folder and must only ever verify it (`build.py --check --install`).
