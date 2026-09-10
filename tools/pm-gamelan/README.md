# Five more measured gamelan models

Installed under **Factory → Physical Models**, alongside PM Saron. Each
instrument is one continuously playable modal physical model. Recordings are
used offline for identification; no PCM, recorded phases, spectral frames or
recorded envelopes are deployed.

| Factory instrument | Exact reference group | Recordings | Modal slots |
| --- | --- | ---: | ---: |
| PM Slenthem | `slenthem-pelog-slenthemmalletpaddedside` | 21 | 16 |
| PM Bonang | `bonangbarung-slendro-bonangmalletwoodenside` | 39 | 32 |
| PM Slenthem Slendro | `slenthem-slendro-slenthemmalletwoodenside` | 21 | 24 |
| PM Kempyang | `kempyang-slendro-bonangmalletwoodenside` | 3 | 24 |
| PM Kethuk | `kethuk-slendro-bonangmalletwoodenside` | 3 | 32 |

All **87 recordings** in these groups participate. The bonang includes the
three recordings labelled **2-broken**, kept as an additional pot on MIDI 74
rather than blended into intact pot 2. Kempyang and kethuk each have one
recorded pot; their keyboard ranges transpose that object. Their interfaces
omit the register-borrowing control, which would have no effect.

Each instrument has **six complete presets**: Reference, Open Ring, Soft
Contact, Hand Muted, Harmonic Metal and Long Bloom. Five teal pages expose all
parameters, with eight main knobs and reactive mechanism diagrams. The
diagrams illustrate a representative mode; they are not audio measurements.

## Playing the reference objects

Recorded tuning = 1 reproduces the measured tuning at these keys. At 0, the
pitch reference follows equal temperament. Between reference keys, modal
coefficients interpolate continuously; outside them, the end object's ratios
transpose. These MIDI assignments are a keyboard convention, not a claim
that the ensemble uses equal temperament.

| Instrument | Source labels, in order | MIDI keys, in the same order |
| --- | --- | --- |
| PM Slenthem | 1, 2, 3, 4, 5, 6, 7 | 50, 52, 53, 56, 57, 58, 60 |
| PM Bonang | 1l, 2l, 3l, 5l, 6l, 1, 2-broken, 2, 3, 5, 6, 1h, 2h | 60, 63, 65, 68, 70, 72, 74, 75, 77, 80, 82, 84, 87 |
| PM Slenthem Slendro | 6l, 1, 2, 3, 5, 6, 1h | 46, 48, 51, 53, 56, 58, 60 |
| PM Kempyang | single pot | 82 |
| PM Kethuk | single pot | 58 |

The labels **medium, harder, hardest** map to normalized velocities
**0.6, 0.8, 1.0**. They are ordinal performance labels, not measured mallet
speeds. Below 0.6, excitation scales the quietest measured strike down to
silence. No softer recordings exist in these selected groups.

Hardness, Contact and Mallet spread change the force spectrum. Decay and
Upper-mode loss control ringing; Bloom changes radiation build-up.
Inharmonic blends measured ratios toward integer harmonics. Register st
borrows another measured register's voicing while retaining the played pitch.
Hand damp adds loss while playing, Release seconds controls key-up damping,
and Hand lift preserves natural ringing after key-up. Stereo spread is an
original spatial treatment, not microphone reconstruction.

## Identification and physical reduction

The workflow follows PM Piano and PM Saron: **measure → reduce to physical
states → calibrate → compare the actual compiled factory DSP**. The five
calibrations describe vibrating metal and its acoustic radiation, not piano
strings or a piano dispersion law.

`families.py` explicitly enumerates the groups, object labels and keyboard
anchors. `analyze.py` records every input filename and SHA256, locates onset,
and pools resolved spectral peaks across strike strengths. The pitch search
uses an object-specific range: the loudest peak of a pot need not be its
pitch reference. Modes below that reference are allowed. Broad room rumble
is excluded from the resonant bank.

Hann-weighted joint sinusoidal projection separates nearby resonances without
double-counting overlapping FFT bands. The projection window expands until
its condition number is below 30. All recorded channels contribute acoustic
energy; estimation phases are discarded.

The wooden slendro slenthem also needs an early 2–60 ms spectral observation.
Its strong, short upper-mode clank can disappear from a sustained spectrum.
Isolated modes above 1 kHz use a 1,024-sample early projection with a 0.03
relative singular-value cutoff; modes separated by less than 80 Hz retain
the longer joint projection. This regularizes unresolved low-mode
combinations during estimation without storing a transient waveform. The
worst low-bar hard impact improved from about 11.6 to 3.6 dB level error in
the first 25 ms; the full 21-file comparison also improved.

For each physical object, every strike shares the **same frequencies,
positive losses, radiation time constants and direct-radiation fractions**.
Only force-to-mode excitation residues depend on velocity. Each mode has the
idealized impulse envelope:

```text
A(velocity) · exp(-r t) · [1 - (1-d) exp(-t/τ)]
```

One damped complex rotation represents the structural mode, followed by a
more heavily damped radiation rotation. Their convex coupling produces the
build-up. Both remain contractive as pitch and damping change. Slenthem's
identified response includes its bar-and-tube system; separate tube geometry,
air-column boundary conditions and modal masses have not been recovered.
Pot mode families likewise are acoustic reductions, not finite-element meshes.
The modal basis and limitations of treating gongs as linear resonators are
discussed in [Normal modes of a small gamelan gong](https://opus.lib.uts.edu.au/bitstream/10453/117446/2/Revised_JASA_Gamelan_submitted.pdf).

The runtime uses one bank per instrument, interpolating the same states and
coefficient surfaces across register and velocity. Mode matching uses frequency
and prominence; absent modes fade through zero excitation. `engine.lisp.in`
is the shared implementation. `build.py` generates standalone DSP, controls,
presets, attribution and capture fixtures. There are no five independently
maintained copies of the algorithm.

Mallet contact is a normalized two-pole force. Reference pole times are
0.30 ms for padded slenthem and 0.06 ms for the wooden mallets; these are force
conventions, not measured collision durations. Their frequency responses are
removed from the residues before applying live contact controls. A squared
acoustic-residue budget prevents hard contact and maximum color from creating
arbitrarily large excitation. This is an acoustic energy proxy, not measured
impact energy in joules. Soft contacts may dissipate energy. At reference
settings the calibrated levels remain unchanged. Modes fade out between
0.40 and 0.47 times sample rate.

Coefficients latch every 16 samples **and on every strike**. A periodic-only
update can lose the start of an off-grid strike, so it is not used. Every
macro dependency is an explicit argument, including the update event, to
preserve execution order after patch-editor writeback.

## Fit and limits

These are restricted acoustic metrics, **not perceptual similarity
percentages or sample-identical reproductions**. Calibration and comparison
use the same recordings; no independent performances were available.

| Instrument | Median absolute RMS error, 25 ms–1 s | Maximum in those windows | Median secondary-mode error |
| --- | ---: | ---: | ---: |
| PM Slenthem | 0.81 dB | 3.31 dB | 2.19 dB |
| PM Bonang | 0.39 dB | 3.69 dB | 0.73 dB |
| PM Slenthem Slendro | 0.32 dB | 2.78 dB | 0.98 dB |
| PM Kempyang | 0.34 dB | 0.73 dB | 0.77 dB |
| PM Kethuk | 0.22 dB | 0.99 dB | 0.47 dB |

`*-comparison.json` contains raw RMS, spectral-cell error, secondary-mode
error and energy outside modal cells for every recording and time window.
Secondary-mode weighting excludes the strongest peak so a near-sine-wave
model cannot hide missing overtones. No clip is individually gain-matched.
The tool enforces regression envelopes for this baseline, including separate
impact, level and secondary-mode checks. Those envelopes are not listening
acceptance criteria.

The first 25 ms remains less exact: maximum level errors are about 6.18,
4.80, 4.04, 1.74 and 3.86 dB, respectively. Individual weak upper modes differ
more than the summary medians. Recording phase, microphone field, room noise,
individual strike irregularity and full nonlinear structural coupling are
not reconstructed. Late 3–5 s discrepancies reach roughly 29 dB in one
bonang clip whose reference RMS is only 0.0000024, and 22 dB in kempyang.
The complete raw levels are retained; there are no per-file fades or added
recording noise to conceal those differences. C2–C8 is signal-tested, but
timbre outside the measured objects is extrapolation.

The **kempul-pelog** candidate was rejected from the factory set. Its attack
and beating vary with strike strength; the tested passive reduction had up
to 10.58 dB level error even after the first 25 ms. The five-model set uses
the distinct wooden slendro slenthem group instead. Rejected measurement
evidence is in `deferred/`; further physical identification is **eseq-mpkt**.
No PM Kempul candidate is installed.

## Validation and reproduction

Validated on macOS ARM64 with pinned **DGenLisp v0.1.17**. Linux's independent
older compiler pin has not been validated for these instruments.

- 1,245 signal checks, including all C2–C8 semitones, velocities, control and
  combined extremes, presets, key-up/hand damping, automation and modulation.
- 87 reference-pitch checks at 44.1, 48 and 96 kHz.
- All 16 strike-clock phases and process partitions of 1, 7, 31, 64, 127 and
  128 samples agree within the recorded tolerances; clock/partition errors
  are zero in the validated runs.
- Actual compiled patch-editor writeback is compared with authored DSP.
- Five factory UI tests exercise every parameter and parameter lock, page
  callbacks, reactive diagrams and finite visible geometry on all 25 pages.
- Production Metal captures cover every instrument and every shared page
  type; images are opened for clipping and visual inspection.
- Fifteen factory host probes cover all five instruments at three rates,
  including Open Ring preset loading. Thirty preset phrases are rendered.
- Generated-C fusion auditing runs after every audition compilation.

The measured one-voice cost at 48 kHz / 128 frames is about 5–7% of one core
for four models and about **19% for Bonang**. These are offline measurements,
not guaranteed real-time polyphony limits. Bonang coefficient-evaluation
profiling is tracked as **eseq-vqh7**; strike correctness must be preserved.

Run from the repository root with dependencies in `requirements.txt`:

```sh
./scripts/fetch_dgenlisp.sh
./scripts/fetch_dgen_toolchain.sh
PYTHONDONTWRITEBYTECODE=1 python tools/pm-gamelan/analyze.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-gamelan/build.py --check
ESEQ_PM_VERIFY_DIR=/tmp/eseq-gamelan-roundtrip cargo nextest run -p eseqlisp --test pm_woodwinds -E 'test(=factory_gamelan_sidecars_preserve_executable_controls)'
PYTHONDONTWRITEBYTECODE=1 python tools/pm-gamelan/verify.py --roundtrip /tmp/eseq-gamelan-roundtrip
PYTHONDONTWRITEBYTECODE=1 python tools/pm-gamelan/compare.py
PYTHONDONTWRITEBYTECODE=1 python tools/pm-gamelan/demo.py
cargo nextest run -p sequencer --bin metal_seq -E 'test(/state_values::tests::pm_woodwind_ui_tests::(slenthem|slenthem_slendro|bonang|kempyang|kethuk)_surface_controls_and_pages/)'
cargo run --bin instrument_probe -- 'factory:Physical Models/PM Bonang' --sample-rate 48000 --midi-note 72 --frames 96000 --gate-frames 12000 --min-peak 0.01 --min-rms 0.001 --json
cargo run -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/pm-bonang-0.lisp --buffer fx --track 0 --width 1800 --height 600 --out tools/pm-gamelan/output/bonang/panel-0.png
```

Pass family slugs to analysis/build/comparison/verification to select a subset.
`compare.py --check-saved` checks existing report hashes and regression
envelopes without rerendering. Omit `build.py --check` only to deliberately
regenerate factory files. After a DSP change, regenerate its authored sidecar
using `eseqlisp::widget_render::patcher::promote_source_to_patch` on a **fresh
copy**, then copy the generated `dsp.layout.json` back. Revalidate compiled
writeback; a parameter-list comparison alone does not prove executable order.

Ignored `output/<family>/` contains every reference A/B (recording first,
0.5-second gap, model second; four seconds each where available), a complete
medium-strike A/B medley, six model-only phrases and panel captures.
`output/five-models.wav` plays the five reference-preset phrases in table order,
with a half-second gap between instruments. No limiter or reverb is applied.

## Source and distribution

Full source credits and **CC BY-NC 4.0** notices are retained in each factory
instrument's `ATTRIBUTION.md`, generated from `ATTRIBUTION.txt`. These are the
Latent Sonorities / memeshift recordings, performed by Bilawa Ade Respati and
recorded by Rabih Beaini. Commercial distribution clearance remains open in
**eseq-uge6**, covering these five instruments and PM Saron. Local modeling
and validation do not establish that permission.
