# PM Tabla — modal tabla kit from the "A5 - Ajanta" sample

One sample-free factory instrument, shipped as **Drums / PM Tabla** (browser
category *Percussion*). It re-creates strokes of the library sample
`A5 - Ajanta.wav` (tags drums, percussion; 4.97 s, mono; the exact file is
`.local/samples/26575811…d905.wav`, verified by SHA256 in `analysis.json`).
The library has other files with the same title; this is the ~5 s percussion one.
The reference is commercial material. The instrument and these tools store
fitted coefficients only (no PCM, recorded frames or envelopes).

The sample has two phrases:

- **0–2.3 s:** a decrescendo dayan roll, ~65 ms per stroke, over a treble
  drum ringing at 168 Hz with near-harmonic partials (299, 446, 593, 739 Hz…).
- **2.3–4.9 s:** separated strokes over a bayan whose pitch the player raises
  with the wrist (meend) from 94 to 132 Hz. The strokes turn to dry, bright
  finger strokes as the bayan fades.

## Playing it

Twelve strokes repeat in **every octave**, one per note name, exactly as in PM
Bongos. The C4 octave plays them at the recorded pitch. Each octave up or down
transposes the whole kit, skin noise included; beyond ±3 octaves the
transposition clamps. Velocity 1 at hardness 0.5 reproduces the recorded level.

| Note (any octave) | Stroke | Reference onset (s) | Contact events (ms) |
| --- | --- | ---: | --- |
| C | Ge Meend | 2.329 | 0, 25.5, 66.5 |
| C# | Ge Rise | 2.664 | 0, 27.5, 63.0 |
| D | Dha | 2.809 | 0, 23.5, 54.0 |
| D# | Dha High | 3.128 | 0, 47.5, 79.5 |
| E | Tun | 3.460 | 0, 20.0, 51.0 |
| F | Te | 3.875 | 0, 65.5 |
| F# | Ti | 4.113 | 0 |
| G | Na Crack | 4.360 | 0 |
| G# | Roll Open | 0.026 | 0, 3.5, 25.5 |
| A | Roll Finger | 0.080 | 0, 27.0 |
| A# | Roll Tin | 0.215 | 0, 24.5, 42.5 |
| B | Roll Bright | 0.466 | 0, 3.5, 18.5 |

The names are listening descriptions borrowed loosely from tabla bols, not
player annotations. The sample holds 29 strokes. The other 17 are played in
comparisons by the key of the same phrase with the closest octave-band modal
energy profile (`build.similar_keys`), at a velocity matched to their early
modal energy.

Controls (14, all on the teal surface; main knobs Hardness, Flam, Decay,
Muffle, Meend, Press, Tune, Output):

- **Hardness / Vel > timbre:** hand contact time. A harder hand redistributes
  a fixed modal energy budget toward upper modes; it never adds energy.
- **Flam / Flam level:** scale the measured spacing and level of every contact
  after the first.
- **Decay / Muffle:** natural ring, and a resting hand whose loss grows with
  frequency.
- **Meend:** replays the measured wrist glide of the bayan modes (1 = as
  recorded, 0 = steady pitch, 2 = double). Only strokes over the gliding
  bayan (C–D#) glide.
- **Press:** shifts only the bayan modes (−5…+7 st) without touching the
  dayan. It is modulatable, so a macro or LFO can play wrist pressure.
- **Tune:** transposes the whole kit ±12 st.
- **Skin / Skin tone / Drive / Tone / Output.** The reference is mono, so there
  is no stereo control; both channels carry the same signal.

Presets: Ajanta (reference), Steady Bayan, Deep Meend, Dry Room, Bright
Fingers, Dusty Record.

## Model

Same engine family as PM Bongos. Per voice there is one bank of **32 damped
complex modal rotations** and a skin-noise band. Each of up to **three contact
events** injects a measured complex residue, with the same normalized
two-pole hand-contact response and skin click/sizzle model. Changes from the
bongos:

- **Meend.** Each mode stores a glide rate (cents/s) and each stroke a glide
  time. Frequencies are `f · 2^((meend · rate · min(t, T) + 100 · press · bayan) / 1200)`,
  recomputed on the 16-sample coefficient clock; after T the pitch holds.
- **Mono output.** The per-mode pan tables and the Stereo control are gone.

## Identification (`analyze.py`, `modal_fit.py`)

1. High-pass at 60 Hz, below the bayan's lowest mode.
2. **Contacts from the 2.5–12 kHz band.** During the roll the ringing drum
   hides onsets in broadband level, so strokes are contact bursts at least
   20 dB above their surroundings. Weaker bursts (≥10 dB) seed the
   extra-contact search, and strokes closer than 30 ms are one gesture.
3. **Shared dayan ring.** A 65 ms roll window cannot resolve modes that ring
   for seconds. The 16 long modes are identified once from the free ring
   after the roll (1.75–2.30 s) and shared, fixed, by every roll stroke;
   16 more modes per stroke are free.
4. **Shared bayan pitch track.** The bayan rings through the second phrase
   while the wrist keeps raising it, so per-stroke glides with independent
   extrapolation made deflation *add* bayan energy. The phrase's bayan pitch
   F(t) is measured once: short-time spectral peaks in 80–150 Hz while it is
   loud (2.34–3.24 s), fitted as a cubic in log frequency (15 cents weighted
   RMS), and held outside that span. A mode below 200 Hz follows
   (F(t)/F(t0))^β with β ∈ [0, 1.2] fitted per mode (0 = a dayan mode). The
   engine's per-stroke glide rate is β times the track's log-linear slope over
   the stroke window.
5. **Sequential deflation and variable projection** as in PM Bongos. Deflated
   rings keep following the shared track past their window.

## Fidelity (`compare.py`, `comparison.json`)

These are the same recordings used for identification: calibration equals
test. They are restricted level/band metrics, not a perceptual score and not
a sample-identical reproduction.

**Isolated keys.** Each installed key is rendered alone through the
production compiler and scored against its own stroke, with every earlier
stroke's fitted ring removed. This measures how faithfully the engine plays
the identified stroke.

| Key | Stroke | 0–10 ms level err (dB) | 10–50 ms (max \|err\|) | 50–100 ms | 0–10 ms band err (dB) |
| --- | --- | ---: | ---: | ---: | ---: |
| C | Ge Meend | −0.2 | 0.3 | −0.2 | 0.6 |
| C# | Ge Rise | −0.2 | 0.2 | −0.1 | 1.7 |
| D | Dha | −0.1 | 0.1 | 0.0 | 1.4 |
| D# | Dha High | −0.2 | 0.2 | 0.0 | 1.6 |
| E | Tun | −0.4 | 0.2 | −0.4 | 1.5 |
| F | Te | −0.7 | 1.1 | −1.1 | 1.5 |
| F# | Ti | −0.6 | 0.8 | −3.6 | 2.1 |
| G | Na Crack | −0.5 | 0.9 | −3.0 | 1.5 |
| G# | Roll Open | −0.4 | 0.0 | — | 0.5 |
| A | Roll Finger | −0.7 | 0.1 | — | 1.9 |
| A# | Roll Tin | −0.3 | 0.1 | — | 0.6 |
| B | Roll Bright | −0.2 | 0.1 | — | 0.3 |

Roll windows end at the next stroke (≈55–70 ms). For Te/Ti/Na the reference
reaches the recording's hiss floor (−52 dB) by 50–100 ms, and the model
deliberately renders no hiss.

**Whole-sample replay.** Every stroke is played at its recorded onset by its
own or its substitute key, so rings overlap as on the record. Installed
strokes stay within ±1 dB in the first 25 ms, except Ge Rise (−1.8/−4.0 dB).
Known limits:

- A stroke's residues partly cancel the previous stroke's ring (the player
  re-strikes and damps). When that previous stroke is a substitute, the
  cancellation is wrong: Ge Rise is 4–8 dB low at 10–50 ms, and the stroke
  after it (substituted) 4–6 dB high.
- Voices never choke each other. Strokes 21–23 are ~8 dB hot at 100–200 ms,
  where the real bayan has been damped by the hand.
- The late, quiet roll strokes played by Roll Tin come out 2–4 dB hot even
  with matched velocity.
- The fit's waveform residual per installed stroke (`analysis.json`) is −15
  to −26 dB for the bayan and roll strokes. For Te/Ti/Na it is −8 dB: most of
  their window is recording hiss after a short, dry stroke.

Listening material (local, not committed) in `.local/pm-tabla/`:

- `sample-ab.wav`: the sample, then the model's replay.
- `ab.wav`: each stroke window, record then model.
- `strokes.wav`: the twelve keys alone, C to B.
- `model.wav`: the model's replay alone.

## Cost (`performance.py`, `performance.json`)

Native process CPU, one voice, 48 kHz, two strikes per second:

| Model | 128-frame blocks | 512-frame blocks |
| --- | ---: | ---: |
| PM Tabla (32 modes) | 0.89% | 0.88% |
| PM Kethuk | 1.00% | 1.01% |
| PM Kempyang | 0.77% | 0.78% |
| PM Bonang | 1.48% | 1.47% |
| PM Saron | 1.09% | 1.11% |

## Checks

- `verify.py` (`verification.json`) at 44.1/48/96 kHz:
  - every key × preset with in-flam retriggers (72 renders per rate);
  - every control at min and max, and all-min / all-max corners (108 per rate);
  - finite audio and state, peak < 4 (max 0.30);
  - 128/512-frame blocks bit-identical;
  - key k+12n is bit-identical to key k with Tune 12n (skin off), and octaves
    beyond ±3 clamp.
- `cargo nextest run -p sequencer -E 'test(pm_tabla_surface_controls_and_pages)'`
  checks all five pages, the eight main knobs and every control's parameter
  and p-lock edits, reading the tracked `model.lisp` / `ui.lisp`.

## Reproduction

Needs the sample in `.local/samples`, the physical-models venv (NumPy, SciPy,
SoundFile, Matplotlib) and the fetched DGenLisp compiler. The analysis takes
~20 minutes.

```sh
P=.local/venvs/physical-models/bin/python
$P tools/pm-tabla/analyze.py
rm -f tools/pm-tabla/noise-calibration.json
for i in 1 2; do $P tools/pm-tabla/build.py && $P tools/pm-tabla/compare.py --calibrate-noise; done
$P tools/pm-tabla/build.py && $P tools/pm-tabla/build.py --check
$P tools/pm-tabla/compare.py
$P tools/pm-tabla/verify.py
$P tools/pm-tabla/performance.py
```

`engine.lisp.in` is the implementation. `build.py` generates the factory
`content/instruments/Drums/PM Tabla/` (`dsp.lisp`, `ui.lisp`, presets and
attribution), plus the `model.lisp`, `ui.lisp` and `model.presets` copies here
that the surface test reads.
