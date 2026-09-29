# PM Bongos — modal hand-drum kit from the bongo-breaks loop

One sample-free instrument, installed in the local user library as
**Physical Models / PM Bongos**. It re-creates every hit of the sampler track
`A4 Bird Of Prey.flac` in `.local/projects/bongo-breaks.json` (the exact file
is `.local/samples/610ba5ad…d06.wav`, verified by SHA256 in `analysis.json`).
The reference is a commercial record, so the instrument stays in `.local`; the
tools here store coefficients only (no PCM, recorded frames or envelopes).

## Playing it

Twelve strokes repeat in **every octave**, one per note name. The C4 octave
(C4 is the default step note, so a plain step plays *Low Open*) plays them at
the recorded pitch. Each octave up or down transposes the whole kit, skin
noise included, by an octave; beyond ±3 octaves the transposition clamps.
Velocity 1 at hardness 0.5 reproduces the recorded level and brightness. For a
flam across heads, either use a flam stroke key or play two keys a few ms apart
(the instrument is polyphonic).

| Note (any octave) | Stroke | Contact events (ms) |
| --- | --- | --- |
| C | Low Open | 0, 29.0, 69.5 |
| C# | Slap + Ghost | 0, 4.0, 48.5 |
| D | Low Ghost | 0, 39.5, 77.5 |
| D# | Pressed Tone | 0, 41.5 |
| E | Mid Open Flam | 0, 11.0, 37.5 |
| F | Mid Open | 0, 22.0, 67.0 |
| F# | Mid Open Short | 0, 30.0, 64.5 |
| G | Mid Ghost | 0, 49.0, 77.5 |
| G# | Low Open 2 | 0, 24.5, 79.5 |
| A | Slap Low Flam | 0, 34.5, 54.5 |
| A# | High Low Muted | 0, 9.0, 16.5 |
| B | High Slap Flam | 0, 14.0, 30.0 |

The loop has 13 gestures. Its second open mid tone ("Mid Open 2") is a
near-duplicate of Mid Open and was dropped to fit twelve notes; in the loop
comparison Mid Open plays that hit (within 1.0–1.8 dB level, 1.2–3.4 dB median
band error).

The names are listening descriptions of the loop, not player annotations. The
record has three pitched heads: low ≈183 Hz (split partner ≈190 Hz), mid
≈273 Hz (partners 277/285 Hz), and a high head whose loudest partials are
≈720/760/1140 Hz. Several hits combine heads within 10–55 ms.

Controls (14, all on the teal surface; main knobs Hardness, Flam, Decay,
Muffle, Tune, Glide, Skin, Output):

- **Hardness / Vel > timbre** — hand contact time; a harder hand redistributes
  a fixed modal energy budget toward upper modes, it never adds energy.
- **Flam / Flam level** — scale the measured spacing and level of every contact
  after the first. Flam 0 lands them all together.
- **Decay / Muffle** — natural ring, and a resting hand whose loss grows with
  frequency.
- **Tune / Glide** — transpose the kit ±12 st; Glide adds a velocity-squared
  tension bend (up to 3%, 28 ms) that the reference barely shows (≈2% on the
  hardest low hit), so it defaults to 0.
- **Skin / Skin tone** — contact click and sizzle level and band.
- **Stereo / Drive / Tone / Output** — Stereo 1 reproduces the measured per-mode
  L/R placement.

Presets: Bird of Prey (reference), Tight Studio, Open Ring, Hard Hands, Soft
Fingers, Dusty Break.

## Model

Per voice: one bank of **32 damped complex modal rotations** (the heads) and a
skin-noise band.

- **Modes.** Each stroke row stores its identified frequencies, loss rates and
  per-mode stereo placement. Rows are selected by key and never interpolated.
- **Contacts.** Each of up to **three contact events** injects a measured
  *complex* residue once. That preserves the recorded relative phase of split
  modes, so the heads beat as they do on the record. Early versions injected
  sine-phase residues only, and pairs such as 183/190 Hz cancelled, costing up
  to 10 dB.
- **Hand contact.** A normalized two-pole force applied as its exact complex
  frequency response ratio (live/reference), evaluated only on contact frames.
- **Skin.** Uniform noise through a per-stroke band-pass. A click (τ 0.3–4 ms)
  plus a sizzle (τ 8–60 ms), both measured in the record's 2.5–12 kHz band,
  where the modes carry almost nothing. Only contacts at detected onsets make
  skin noise; finger-bounce re-excitations do not.

All vector coefficient work runs on event clocks: the 16-sample coefficient
tick for tune/glide/decay/muffle/width, and contact frames for excitation.
Per sample, each mode costs a rotation, one injection and two output taps.

### What the extra contact events are

The greedy search adds a contact wherever it lowers the residual by ≥0.8 dB.
In the flams they coincide with detected onsets. In the open tones, recurring
~22–40 ms and ~65–80 ms re-excitations appear on both heads. A single global
room echo was tested and rejected: the error surface over reflection delays was
flat. So they are kept as measured per-stroke contact behaviour (hand staying
on or bouncing off the skin), not claimed as separate strikes.

## Identification (`analyze.py`, `modal_fit.py`)

1. High-pass at 110 Hz (the record's 7 Hz rumble and 68 Hz hum sit below every
   head mode). Detect onsets; onsets closer than 45 ms form one gesture.
2. **Sequential deflation.** The low head rings ~0.4 s and hits are ~0.28 s
   apart, so each hit's window contains earlier tails. The ring before the
   first onset is fitted first. After each hit is fitted, its modal response
   is subtracted from the rest of the loop. Without this, ghost notes were
   fitting the previous stroke's ring.
3. **Variable projection** on the 16.25 kHz decimated stereo window. Modal
   amplitudes of all strikes and both channels are linear least squares;
   frequencies (±60 cents) and loss rates are optimised. The loss floor rises
   0.025 /s per Hz above 1 kHz, because lightly damped high "modes" were
   fitting the record's hiss.
4. Contact click/sizzle levels and decays, with the pre-onset hiss removed in
   power.

## Fidelity (`compare.py`, `comparison.json`)

The model re-plays the whole loop twice from isolated key renders. Each hit
window of the second pass is scored against the high-passed reference, so
earlier strokes and the previous bar ring in as they do on the record.

These are the same recordings used for identification: calibration equals
test. They are restricted level/band metrics, not a perceptual score and not
a sample-identical reproduction.

| Key | Stroke | 0–10 ms level err (dB) | median later window | max window | median band err (dB) |
| --- | --- | ---: | ---: | ---: | ---: |
| 60 | Low Open | 0.1 | 0.5 | 0.5 | 0.5 |
| 61 | Slap + Ghost | 0.1 | 0.1 | 0.1 | 0.4 |
| 62 | Low Ghost | 0.7 | 0.4 | 0.7 | 1.9 |
| 63 | Pressed Tone | 1.3 | 0.3 | 2.0 | 0.7 |
| 64 | Mid Open Flam | 0.1 | 0.2 | 1.1 | 0.7 |
| 65 | Mid Open | 0.2 | 0.2 | 0.8 | 0.6 |
| 66 | Mid Open Short | 0.0 | 0.4 | 1.2 | 0.6 |
| 67 | Mid Ghost | 0.6 | 1.9 | 2.8 | 1.1 |
| 65 | Mid Open 2 → Mid Open | 1.0 | 1.4 | 1.8 | 1.9 |
| 68 | Low Open 2 | 0.3 | 0.2 | 0.3 | 0.2 |
| 69 | Slap Low Flam | 0.2 | 0.4 | 0.6 | 0.9 |
| 70 | High Low Muted | 0.4 | 1.3 | 4.2 | 1.2 |
| 71 | High Slap Flam | 0.1 | 1.0 | 1.1 | 0.8 |

Windows are 0–10, 10–25, 25–50, 50–100, 100–200 and 200–320 ms (until the next
hit). Band error is taken over octave bands within 30 dB of the loudest.

Known limits:

- High Low Muted's 100–200 ms window is 4 dB low: the densest two-head hit,
  late fit residual −8 dB.
- The two ghost notes sit near the record's noise floor.
- The final hit is cut by the end of the file at 50 ms, so its long modes are
  extrapolated.
- The record's constant vinyl hiss is deliberately not synthesized.
- Mic phase differences between channels are reduced to one mono phase plus a
  per-mode level pan.
- The waveform residual of the fit is −10 to −28 dB per hit (`analysis.json`).

Listening material (local, not committed) in `.local/pm-bongos/`:

- `loop-ab.wav` — the record twice, then the model loop twice.
- `ab.wav` — each hit window: record, then model.
- `strokes.wav` — the twelve strokes alone, C to B.
- `loop-spectrograms.png` — side-by-side spectrograms.

## Cost (`performance.py`, `performance.json`)

Native process CPU, one voice, 48 kHz, two strikes per second, on the same
machine and run:

| Model | 128-frame blocks | 512-frame blocks |
| --- | ---: | ---: |
| PM Bongos (32 modes) | 1.36% | 1.36% |
| PM Bonang | 1.44% | 1.50% |
| PM Saron | 1.12% | 1.12% |
| PM Kethuk | 0.95% | 0.95% |
| PM Kempyang | 0.78% | 0.78% |

Absolute numbers move with machine load; the ratios are stable. With 24 modes
it measured ≈1.0%, but High Low Muted then kept only 93% of its identified
energy.

## Checks

- `verify.py` (`verification.json`) at 44.1/48/96 kHz:
  - every key × preset with in-flam retriggers;
  - every control at min and max, and all-min / all-max corners;
  - finite audio and state, peak < 4;
  - 128/512-frame blocks bit-identical;
  - key k+12n is bit-identical to key k with Tune 12n (skin off), and octaves
    beyond ±3 clamp.
- `host-probes.json`: every preset at keys 60/66/72 through the production
  host compile/load path (`target/debug/instrument_probe`); keys 36–108 were
  also probed after the octave layout (finite, peaks 0.12–0.47).
- `cargo nextest run -p sequencer -E 'test(pm_bongos_surface_controls_and_pages)'`
  checks all five pages, the eight main knobs and every control's parameter and
  p-lock edits, reading the tracked `model.lisp` / `ui.lisp`.

## Reproduction

Needs the bongo-breaks sample in `.local/samples` and the physical-models venv
(NumPy, SciPy, SoundFile, Matplotlib). The analysis takes ~12 minutes.

```sh
P=.local/venvs/physical-models/bin/python
$P tools/pm-bongos/analyze.py
rm -f tools/pm-bongos/noise-calibration.json
for i in 1 2; do $P tools/pm-bongos/build.py && $P tools/pm-bongos/compare.py --calibrate-noise; done
$P tools/pm-bongos/build.py && $P tools/pm-bongos/build.py --check
$P tools/pm-bongos/compare.py
$P tools/pm-bongos/verify.py
$P tools/pm-bongos/performance.py
```

`engine.lisp.in` is the implementation; `build.py` generates the installed
`dsp.lisp`, `ui.lisp`, presets and attribution, plus the tracked `model.lisp`,
`ui.lisp` and `model.presets` copies here.
