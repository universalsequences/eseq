# PM Saron

One playable struck-bar physical model, calibrated from all **35
saron-pelog-saronmallet** recordings in the Latent Sonorities pack: seven pélog
bars at five strike strengths. Installed as **Factory → Physical Models →
PM Saron**. The instrument contains resonant coefficients, not samples,
recorded waveforms, phases, or stored spectral frames.

This carries over PM Piano's measurement → physical reduction → calibration
→ compiled comparison workflow. Its resonator and excitation model are
different: measured inharmonic metal-bar modes, a short mallet force, and
frequency-selective resonance build-up. There are no piano strings, felt
hammer assumptions, unison-string banks, or piano dispersion law.

## Playing

The reference preset is **Pelog Saron**. The complete bank also includes
**Open Bronze, Soft Mallet, Muted Bronze, Harmonic Metal, and Low Bars**.
Each recalls all 19 parameters of the same engine. Low Bars also transposes
the played register down two octaves.

| Bar | MIDI key | Measured fundamental |
| --- | --- | --- |
| 1 | 74 / D5 | 596.258 Hz |
| 2 | 75 / E♭5 | 639.010 Hz |
| 3 | 77 / F5 | 698.854 Hz |
| 4 | 80 / A♭5 | 814.907 Hz |
| 5 | 81 / A5 | 881.195 Hz |
| 6 | 82 / B♭5 | 933.762 Hz |
| 7 | 84 / C6 | 1032.208 Hz |

Recorded tuning = 1 reproduces this ensemble's tuning at these keys. At 0,
the fundamental follows equal temperament. Intermediate keys continuously
interpolate the bar's coefficients. Outside the measured register, the end
bar's ratios continue at the played pitch. C2–C8 is signal-tested; timbre
outside the seven recorded bars is an extrapolation, not measured accuracy.

The labels softest, soft, medium, harder and hardest map to normalized
velocities **0.2, 0.4, 0.6, 0.8 and 1.0**. These labels do not specify actual
mallet speeds or MIDI velocities, so the evenly spaced mapping is a playing
convention. Timbre and level blend between them. Below 0.2, the quietest
strike's excitation scales continuously to silence.

| Control | Effect |
| --- | --- |
| Hardness / Contact | Change the bandwidth and duration of the mallet force. |
| Mallet spread | A wider force footprint excites short spatial wavelengths less. |
| Vel > timbre | At 1, uses all measured strike colors; at 0, velocity scales a fixed medium-strike color. |
| Decay / Upper-mode loss | Scale natural ringing and frequency-dependent damping. |
| Bloom | Scales the measured resonance build-up time, separately from decay. |
| Inharmonic | At 1, measured modal spacing; at 0, harmonic spacing; above 1, exaggerates the deviations. |
| Register st | Borrows another register's modal voicing while retaining the played fundamental. |
| Spectral color | Tilts the modal excitation around the fundamental. |
| Hand damp | Applies continuous extra damping, including while a note is held. |
| Release seconds | Extra key-up damping, expressed as a fundamental T60. Natural losses remain active. |
| Hand lift | At 1, key-up leaves the bar ringing naturally. It does not override Hand damp. |
| Stereo spread | Places modes across the panorama; this does not reconstruct the recording microphones. |
| Drive / Tone Hz / Output | Final saturation, low-pass filtering and level. |

The five teal display pages follow real reactive parameter bindings. The
curves illustrate the model mechanisms; they are not live audio measurements.
The bar diagram uses representative bar-3 coefficients.

## Identification and model

`analyze.py` finds spectral lines that are resolved in at least three strike
strengths. It rejects low-frequency room rumble and broad noise. Joint,
Hann-weighted sinusoidal least squares separates nearby resonances without
counting an overlapping FFT band twice. Its condition number is recorded for
every bar (below 3 in this set). The phases needed during estimation are
discarded; only stereo-energy amplitudes are retained.

All five strikes of each bar jointly determine a **shared** modal loss and
radiation response. Excitation residues are the only fitted quantities that
vary between those five strikes. Noise-floor thresholds limit the fitting
window, and a positive loss floor prevents an endless fitted noise tail.

`calibrate.py` matches modal slots between adjacent registers by frequency
and prominence. Missing modes fade to zero excitation; their frequencies
continue from identified neighbors. The runtime has 24 modal slots, with
9–23 identified lines per reference bar. Coefficients interpolate across
seven registers and five excitation strengths. No recorded envelope arrays
or per-recording oscillator programs are selected at runtime.

Each bar mode is a damped complex rotation. A second, more heavily damped
rotation at the same frequency models its radiation build-up, with a direct
path in parallel. The nominal impulse envelope is

```text
A · exp(-r t) · [1 - (1-d) exp(-t/τ)]
```

Here `r > 0` is natural loss, `τ > 0` is the build-up time and `0 ≤ d ≤ 1`
is direct radiation. The implementation is a causal recurrence. It can be
retriggered and damped while ringing. The rotations remain contractive
during tuning changes; the radiation stage is a convex, dissipative update.
This is an identified modal reduction, not a finite-element reconstruction
of the bars, a nonlinear mallet collision solver, or proof of their exact
geometry/material properties. See the acoustic study
[Comparative acoustical and psychoacoustical analyses of gamelan instrument tones](https://www.jstage.jst.go.jp/article/ast1980/14/6/14_6_383/_pdf)
for the bar-mode basis of saron analysis.

A normalized two-pole impulse approximates mallet contact. Its nominal
60 µs pole time is a reference force convention, not a measured collision
duration. Its discrete magnitude response is removed from the fitted
residues before applying live Hardness and Contact. Modal weights fade out
between 0.40 and 0.47 times the sample rate to avoid folded high resonances.

Controls and coefficients latch every 16 samples **and immediately on a
strike**. Periodic-only coefficient updates were rejected: an off-grid hit
could otherwise lose part of its force or use the previous velocity's
weights. These event-aware latches compute their inputs at frame rate.
All macro dependencies are explicit inputs so graph writeback preserves
their evaluation order.

## Accuracy and limits

The comparisons use the actual compiled factory instrument. Across all 35
references in the 25–120 ms, 120–400 ms and 400 ms–1 s windows, median absolute
RMS level error is **0.69 dB**. Per-window medians are **0.60, 0.77 and 0.62 dB**;
the largest error in those windows is **2.67 dB**. The first 25 ms is less
accurate: median **1.39 dB**, maximum **5.58 dB**.

`reference-comparison.json` also reports normalized modal-band error and a
separate upper-mode error relative to the fundamental. The latter prevents
the very strong fundamental from hiding an overtone mismatch. Both are
restricted spectral metrics, not perceptual similarity percentages. Read
them together with raw RMS levels, energy outside modal bands and the A/Bs.
The upper-mode metric's median is **0.90 dB** and maximum **7.72 dB** over
25 ms–1 s, excluding windows without sufficiently strong overtones. The
comparison command enforces separate level and overtone gates; a quiet model
or a fundamental-only sine cannot pass by exploiting spectrum normalization.

This is **not sample-identical**. Recording phase, microphone positions,
stereo room response, background noise and individual strike irregularities
are not reproduced. The five recordings' natural decay is not perfectly
consistent. The 2–3 s window differs by about 11.7 dB for bar 2 softest and
10.9 dB for bar 7 hardest, whose late recorded levels are already very low.
Recording treatment or performance damping may contribute; the cause has not
been identified from the recordings alone. There are no per-file timed fades
in the model to conceal those discrepancies.

Calibration and reference comparisons use the same 35 files. They demonstrate
fit to the requested set, not accuracy on independent recordings, other
sarons, or unrecorded mallets and strike locations.

## Reproduce

Run from the repository root with Python, NumPy, SciPy and SoundFile. The
validated environment used NumPy 2.1.3, SciPy 1.18.1 and SoundFile 0.14.0.

```sh
./scripts/fetch_dgenlisp.sh
./scripts/fetch_dgen_toolchain.sh
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/analyze.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/calibrate.py --check
ESEQ_PM_VERIFY_DIR=/tmp/eseq-saron-roundtrip cargo nextest run -p eseqlisp --test pm_woodwinds -E 'test(=factory_saron_sidecar_preserves_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/verify.py --roundtrip '/tmp/eseq-saron-roundtrip/PM Saron/dsp.lisp'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/verify.py --pitches-only
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/compare.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-saron/demo.py
cargo nextest run -p sequencer --bin metal_seq -E 'test(=state_values::tests::pm_woodwind_ui_tests::saron_surface_controls_and_pages)'
cargo run --bin instrument_probe -- 'factory:Physical Models/PM Saron' --sample-rate 48000 --midi-note 77 --frames 96000 --gate-frames 12000 --min-peak 0.01 --min-rms 0.001 --json
cargo run -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/pm-saron.lisp --buffer fx --track 0 --width 1800 --height 600 --out tools/pm-saron/output/panel-mallet.png
```

Omit `--check` to deliberately regenerate the DSP tables. A DSP change also
requires a new authored graph sidecar: use
`eseqlisp::widget_render::patcher::promote_source_to_patch` on a fresh copy of
the source, then replace `dsp.layout.json` with that generated sidecar.
The roundtrip test exports the actual editor writeback; `verify.py` compiles
and compares its audio. Never assume matching parameter names proves the
saved graph remains executable.

The analysis expects the user's reference files under
`samples-to-analyze/gamelan/`; they are not runtime assets. Every input file's
SHA256 is recorded and checked. The five host-probe commands and their results
are retained in `host-validation.json`.

## Recorded validation

On macOS ARM64 with the repository's pinned **DGenLisp v0.1.17**:

- **272 signal checks**: C2–C8, velocity continuity/silence, each control's
  extremes, combined extremes, all six presets, natural/hand/key-up damping,
  44.1/48/96 kHz, automation, host modulation, idle silence and gate-only onset.
- All 16 strike-clock phases agree exactly. Process-call partitions of
  1, 7, 31, 64, 127 and 128 samples agree exactly, including off-grid changes.
- **21 pitch checks** across seven bars and three sample rates agree with
  the calibrated frequencies within 0.001 cent. This measures implementation
  accuracy against the fitted frequencies, not uncertainty in their estimation.
- The authored sidecar test passes; compiled editor writeback differs by less
  than **0.000001 peak per sample** during notes, key-up and control changes.
- The factory UI test covers all 19 parameter bindings, parameter locks,
  page callbacks, reactive diagrams and finite/nonzero visible geometry.
  All five production Metal captures were opened and inspected.
- Five production host probes pass, including preset loading. The generated-C
  fusion audit runs after every audition compilation.

The measured one-voice cost at 48 kHz / 128 frames was about **6.9% of one
core** on the recorded Mac. This is offline process CPU time, not a certified
polyphony limit or worst-case real-time guarantee. Linux uses an independent
older compiler pin and has not been validated for this instrument.

## Listening files

Generated files live under ignored `output/`:

- `seven-bars-ab.wav`: medium strikes, bars 1–7. For each bar: **recording
  first**, 0.5 s silence, **model second**. Relative levels are preserved.
- `bar-N-STRENGTH-ab.wav`: the same format for every individual reference.
- `phrase-pelog-saron.wav`: a short phrase made entirely by the model.
- `six-voices.wav`: the same phrase through all six presets, in bank order.
- `panel-*.png`: production captures of the five display pages.

No per-clip peak normalization, compressor, limiter, reverb or sample layer
hides the differences. Demo peaks are checked to stay below full scale.

## Source attribution and distribution

Source: **Latent Sonorities / memeshift**, performed by **Bilawa Ade Respati**,
recorded by **Rabih Beaini**, using RBI's Javanese ensemble in Berlin.
Full credits and the **CC BY-NC 4.0** source notice are retained in
[`PM Saron/ATTRIBUTION.md`](../../content/instruments/Physical%20Models/PM%20Saron/ATTRIBUTION.md).
Commercial factory distribution has not been cleared; this is tracked in
Beads as **eseq-uge6**. Local modeling and validation do not establish that
permission. No attribution or license claim is made for the other samples
in the large gamelan folder, which this work does not use.
