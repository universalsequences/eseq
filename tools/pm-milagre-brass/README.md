# PM Milagre Brass: the horn that opens "Bodas"

One sample-free factory instrument, **Physical Models / PM Milagre Brass**
(browser group *Brass*). It is identified from the first 7.9 s of
"Bodas (Ao Vivo)" (Milton Nascimento, *Milagre Dos Peixes (Ao Vivo)*, track
1-02). In those seconds one valveless lip-reed horn plays alone, until the band
enters at 7.97 s. The recording is commercial: the instrument and these tools
store fitted coefficients and harmonic-amplitude measurements only (no PCM,
phase or full spectrum). Listening material is written to `.local/` only.

## What the horn plays

Every note is a partial of one bore whose partial spacing is 112.0 Hz. That
is an A2 about 31 cents above A440 tuning.

| Time (s) | Note | Partial | Measured Hz | Cents from ET | Articulation |
| --- | --- | ---: | ---: | ---: | --- |
| 0.045-2.075 | A2 | 1 (pedal) | 112.00 | +31 | breath dip at 1.93 |
| 2.230-2.440 | A2 | 1 | 111.46 | +23 | tongued |
| 2.640-2.865 | A2 | 1 | 111.61 | +25 | tongued |
| 2.995-3.345 | C#5 | 5 | 566.17 | +36 | tongued |
| 3.375-3.470 | E5 | 6 | 668.94 | +25 | tongued |
| 3.500-5.150 | A2 | 1 | 111.79 | +28 | tongued |
| 5.150-6.855 | E4 | 3 | 335.46 | +30 | slurred |
| 6.855-7.790 | A2 | 1 | 111.58 | +25 | slurred, crescendo |

The A2 is a pedal tone. Its fundamental sits ~30 dB under its 3rd harmonic,
and harmonics 3 and 6 dominate. The "other notes" are partials 3, 5 and 6 of
the same horn, the way a natural horn or a berrante is played.

## Model (`engine.lisp.in` + `build.py`)

- **Driven lip valve.** The lips buzz at the note frequency, so pitch is
  exact. The opening is `max(0, env*lips + buzz*be^curve*(f/f_horn)^-register*sin)`.
- **Bernoulli flow** against the bore's input impedance. The flow
  `u = h sign(dp) sqrt|dp|` is solved in closed form each sample.
- **Modal bore.** 16 resonances with identified frequency ratios and peak
  heights, a Q law, a bell cutoff and the characteristic impedance. It is not
  harmonic, and that is the tone:
  - mode 1 sits at 0.73x, the classic flat first brass resonance, which is why
    the pedal has no fundamental;
  - mode 4 sits at 4.81x (−70 cents), so the pedal's 4th harmonic misses it;
  - mode 6 is +9 dB, which gives the bright 6th harmonic.
  - Ratios 3, 5 and 6 are fixed by the sounding notes.
- **Natural horn.** A note sounds as the nearest partial of **Horn key**. The
  bore is scaled so that partial lands exactly on the played pitch. A slur
  changes the lips while the bore stays put. Slurs glide in log frequency;
  tongued notes start on pitch.
- **Register law.** `be = breath * 0.232 * (f/f_horn)^-1.18`. Higher partials
  speak with less breath, so one Breath value gives one dynamic on every note.
  The recorded mezzo-forte is Breath ≈ 0.6.
- **Body.** A 12-section RBJ peaking cascade fitted to the shared
  radiation/mic/room response, scaled by **Body** (0 = dry horn).
- **Hall.** Four combs and two allpasses, with T60 1.5 s fitted on the gaps.
- **Output.** `2 tanh(x)`, transparent at playing levels. The returning bore
  pressure is bounded, so extreme settings saturate instead of diverging.

## Identification

1. `analyze.py` reads the mp3 (via ffmpeg). It writes `analysis.json`: the
   hand-checked score, each note's pitch, and per 10 ms frame the harmonic
   amplitudes and pitch, plus the pre-roll noise floor.
2. `fit_bore.py` compiles a probe with every constant as a param. It then:
   - blows the probe with slow breath ramps in each regime;
   - fits a shared body response G(f) and a per-frame breath jointly;
   - searches the lip constants (Nelder-Mead), the mode ratios (cents) and the
     mode heights (dB), with notes weighted equally.

   Result: 4.9 dB RMS per-harmonic error in the search, 5.1 dB on the finalize re-check (stage 1). The reference itself
   wanders 2.8 dB around its own 200 ms average, so that is roughly the floor
   for a smooth model. A second per-frame control (lip opening) gained only
   0.1 dB, so the remainder is not expression a p-lock could fix.
   `--finalize` fits the body cascade (max deviation 0.1 dB) and writes
   `fit.json`.
3. `fit_phrase.py` plays the phrase exactly as the sequencer will
   (`performance.py`). It fits the Breath p-lock on every step by damped
   iterative learning control on frame loudness. It also fits attack,
   release, breath smoothing, slide and hall with Nelder-Mead on the
   whole-phrase error. It writes `phrase.json`.
4. `make_script.py` writes `phrase.lisp`.

## Fidelity (`phrase.json`, same recording as calibration)

Per-harmonic RMS error over every 10 ms frame. Harmonics are clamped to the
recording's noise floor or to 40 dB below the frame's peak.

| Note | dB |
| --- | ---: |
| A2 long | 5.98 |
| A2 short ×2 | 5.42 / 5.50 |
| C#5 | 6.16 |
| E5 | 4.94 |
| A2 | 5.34 |
| E4 | 3.73 |
| A2 swell | 5.67 |
| gaps (hall) | 6.88 |

In-note loudness tracks within **0.8 dB RMS (median 0.23 dB)**.

Known limits:

- Articulations carry a short broadband burst, 3–8 dB above the local
  >3 kHz level for ~20 ms.
- The model's harmonics above ~2 kHz on E4 stay present where the recording
  sinks into its hiss.
- The recording's hum, hiss and audience are not modelled.
- Breath noise (`air`) fitted to zero.

## Playing the phrase

`phrase.lisp` is a scratch-buffer script (C-x C-b). Set the track up first:

1. Put PM Milagre Brass on the current track with preset *Milagre*.
2. Set BPM 100, timebase 1/32 and length 128.
3. Turn Poly off and set Trig to legato.

The script then writes:

- 8 notes, each with Delay micro-timing and Duration. Slurred notes overlap
  the next note by one step.
- A Breath lock and a Tune lock (the player's intonation, cents) on every
  step.

## Checks

- `verify.py` (`verification.json`), at 44.1/48/96 kHz:
  - the phrase, every preset over A1–A5, every control at min and max, and
    all-min / all-max corners;
  - all output finite, peak ≤ 2;
  - 128/512-frame blocks bit-identical;
  - ~1.6% of one core with the Python host loop (an upper bound).
- `cargo nextest run -p sequencer -E 'test(milagre_brass_surface_controls_and_pages)'`
  checks that every parameter is reachable on one of the six pages.
- `build.py --check` confirms the shipped files match the generator.

## Reproduction

```sh
P=.local/venvs/physical-models/bin/python
$P tools/pm-milagre-brass/analyze.py            # needs the mp3 in ~/Music (or pass its path)
$P tools/pm-milagre-brass/fit_bore.py --rounds 4  # ~40 min
$P tools/pm-milagre-brass/fit_bore.py --finalize
$P tools/pm-milagre-brass/build.py
$P tools/pm-milagre-brass/fit_phrase.py         # ~8 min, rebuilds
$P tools/pm-milagre-brass/make_script.py
$P tools/pm-milagre-brass/verify.py
$P tools/pm-milagre-brass/compare.py            # .local/pm-milagre-brass/ab.wav, compare.png
```
