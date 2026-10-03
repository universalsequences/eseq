# PM Nylon Guitar: plucked strings identified from a recorded guitar intro

A local-only instrument (`.local/instruments/Physical Models/PM Nylon Guitar`).
It is identified from the opening ~15 s of solo nylon guitar in "Nascer", a
commercial recording (`~/Downloads/Nascer.mp3`). Only fitted string and body
coefficients are stored: no PCM, recorded phase or spectral frames.

## Model

Each voice is one string with 32 stiff-string partials. The excitation of
partial k comes from a recorded pluck of the nearest recorded pitch
(C#2 D#2 G#2 F3 G3 C4 D#4; 40 plucks in all):

    a_k = L sin(pi k beta) k^-alpha exp(-(f_k/fc)^2) exp(h(f_k) + d_k)

- **Per take:** L, beta (pluck position), alpha (finger tilt), fc (fingertip/nail
  roll-off) and d_k (measured per-partial detail).
- **Shared:** h, the measured body/radiation EQ.

Further pieces:
- **Loss:** s0(pitch) + saturating b f², plus a calibrated half-octave trim.
- **Body-coupled partners of partials 1–2:** the measured fast poles beside the
  string poles.
- **Body bank:** 8 resonances (air ~100 Hz, top ~200 Hz…) driven by the bridge,
  plus a pluck knock.
- **Finger contact noise** and a ~1 ms release ramp.

Velocity, Take and Humanize choose which recorded pluck plays. Timbre knobs
redistribute the take's energy and never add to it.

## Pipeline

```sh
P=.local/venvs/physical-models/bin/python
$P tools/pm-guitar/analyze.py          # onsets, notes, joint per-partial pole fits -> analysis.json
$P tools/pm-guitar/fit_model.py --cap 15   # pluck/EQ/loss/coupling model -> model.json
rm -f tools/pm-guitar/calibration.json
for i in 1 2 3 4 5; do $P tools/pm-guitar/build.py && $P tools/pm-guitar/compare.py --calibrate; done
$P tools/pm-guitar/build.py && $P tools/pm-guitar/build.py --check
$P tools/pm-guitar/compare.py          # per-take band/level errors -> comparison.json
$P tools/pm-guitar/verify.py           # stability, block invariance, CPU -> verification.json
$P tools/pm-guitar/render.py           # .local/pm-guitar/scale.wav (model only)
```

## Fidelity (compare.py)

Each recorded pluck is played alone by the compiled engine at its own pitch.
It is compared with the record's "new energy" in that window (post-pluck band
power minus what was ringing just before), with calibration equal to test.
Median band errors:

- **30–100 ms:** within ±0.4 dB from 70 Hz to 4.5 kHz.
- **100–250 ms:** within ±1.6 dB up to 1.1 kHz. Above that the record keeps
  2–4.5 kHz energy the model lacks (room and other strings, not chased).
- **Level:** take-to-take spread about ±2 dB.

## Cost

About 1.6% of one core per voice (48 kHz, 128-frame blocks).
