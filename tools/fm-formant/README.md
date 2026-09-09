# FM Formant implementation candidate

This is a playable four-voiced/four-unvoiced FM/formant engine built with the
published macOS DGenLisp compiler. **It does not yet meet the factory specification.**
It is deliberately outside `content/instruments`: the complete voice misses the
CPU gate, and structured program editing/persistence needs host implementation.

The installed audition copy is **Library → Experiments → FM Formant**. Its
186 controls, eight performance controls, separate voiced/noise envelopes,
formant motion, forward PM, explicit one-sample feedback, and stereo output are
real DSP. The 32 initial presets are starting voicings, not a listening-approved
factory bank. The macOS compiler pin is updated; host UI layout coverage accompanies the candidate.

## Four-operator architecture (current)

The instrument now compiles exactly four voiced operators, four independent noise
bands, six forward PM routes and sixteen one-sample feedback routes. E–H are
absent from DSP, parameters, motion and UI. Motion schema version 2 requires four
entries per frame and compiles to 29 channels; old eight-entry assets are rejected.
All 32 presets were regenerated for the smaller graph, with stereo spread restored
and Metal feedback moved to D→A. Algorithm choices are Parallel, two stacks, Chain.

Matched 48 kHz / 128-frame single-voice medians using the same published compiler:

| Waveform workload | Eight operators | Four operators |
|---|---:|---:|
| All formant | 627.92 µs | 308.88 µs |
| All sine | 183.07 µs | 93.01 µs |

This is roughly 51% and 49% less processing respectively, not a measured host
polyphony result. Raw randomized timing batches and source hashes are in
`results/four-architecture.json`. `results/four-voice.json` records all 96 preset
renders, seed repeatability and 32/64/128/256 block-size checks. The installed
host probe passes with Metal 2 (peak 0.08214, RMS 0.01174). The 12-voice factory
CPU target is still not certified. Measurements below describe the earlier
**eight-operator** development stages and are retained as historical evidence.

## Source and rebuild

`kernel.lisp` is the reusable finite-series formant primitive. `voice.lisp` owns
phase, smoothing, deterministic band-noise generation, resonators, and output
helpers. `build_instrument.py` produces standalone readable DGenLisp and a native
motion asset; `build_ui.py` copies the authored `ui.lisp` panel. `presets.py` authors
complete scalar preset states. `candidate/` is the generated audition artifact;
edit the source modules and regenerate rather than maintaining two DSP versions.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/fm-formant/build_instrument.py \
  --output /tmp/fm-formant-candidate
```

Pass `--motion path/to/motion.json` to supply an original motion. The version 2
schema is illustrated by [candidate/motion.json](candidate/motion.json). It
contains 2–64 ordered frames, four voiced/noise entries per frame, seven named physical fields
per pair, and shared pitch in semitones. The generated tensor is channel-major,
with frequencies and widths stored in log space. Both files are included so
the editable representation remains separate from compiled storage. Nonclosed
assets expose only manual and note one-shot modes; looping/one-bar modes are
available only when both endpoint frames match, including pitch.

Scalar presets can be generated with `presets.write_bank(directory, defaults)`;
the complete validation command below does this automatically. Source generation
is an offline authoring tool, not a substitute for the missing in-app motion editor
and atomic program-publication protocol.

## Formant law

For positive harmonics `h` below Nyquist, the kernel uses the spectral envelope

```text
x = 2 * abs(h*f0 - Fc) / B
A(h) = S*exp(-ln(2)*x)
     + (1-S)/2 * [exp(-ln(4)*x) + exp(-ln(4/3)*x)]
```

Every component combination is 1 at the center and 0.5 at `Fc ± B/2` before
harmonic sampling and Nyquist tapering. Thus width and skirt have separate
definitions. A two-harmonic cosine taper approaches Nyquist continuously; DC
and harmonics at/above Nyquist are omitted. The amplitude bound is the sum of
positive coefficients, floored at 1. PM rotates the resulting quadrature pair
without moving the repetition clock.

The two sides of each exponential spectrum are finite geometric series. Their
closed forms evaluate in constant work, without a per-partial runtime loop,
oversampling, or an unbounded set of overlapping bursts. The factored denominator
and small-argument `1-exp(-x)` calculation avoid catastrophic cancellation near
phase zero. This is an original finite-series construction, **not identified
Yamaha DSP**. It is an implementation alternative to the spec's carrier/window
direction, with an exact harmonic reference for its own law.

The unmodulated kernel excludes out-of-band harmonics by construction. **Dynamic
PM and feedback can still generate aliasing.** Finite rendering and arithmetic
agreement do not establish alias-free FM or perceptual quality.

## Measurements

Evidence under `results/` was collected on this macOS/M1 Max session using the
compiler hash recorded in each report. `instrument_probe` used the existing
debug binary and the normal published-compiler compile/load/init path; Rust was
not rebuilt because no host code changed.

| Check | Observed result |
|---|---|
| Kernel vs explicit harmonic sum | 72 cases, 44.1/48/96 kHz; maximum sample error 3.654e-6 |
| Kernel workload | About 6.97 µs per 128 frames at 96 kHz; two output channels include phase diagnostic |
| Complete voice presets | 32 presets × three pitches; finite output and release-tail checks passed |
| Block-size comparison | Identical tested dry output at 32/64/128/256 frames |
| Repeated seeded render | Identical tested complete output |
| Host smoke render | Finite audio and state; peak 0.16878, RMS 0.02133 at the probe's default 44.1 kHz |
| Complete voice workload | Final median 579.018 µs per 128 frames at 48 kHz; initial 581.996 µs |
| Native motion validation | Round-trip and invalid-data rejection tests pass |
| Custom UI | Production capture inspected; geometry and parameter reachability checked across 24 pages |

The complete-voice figure is about **21.7% of one core for one voice**, before
full-host polyphonic scheduling and other instruments/effects. It fails the
spec's 12-voice target. Do not present a multiplication of this microbenchmark
as a measured polyphonic result. Timing batches include Python/ctypes overhead;
their spread is not a p95 callback measurement. Exploratory feedback timings
were noisy and are not used for release claims.

## Reproduce validation

These commands use the pinned installed compiler and toolchain. The standalone
audition harness additionally needs an explicit path to its binary-audit script.
The normal host probe performs its own audit.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s tools/fm-formant -p test_motion.py

PYTHONDONTWRITEBYTECODE=1 python3 tools/fm-formant/validate_kernel.py \
  --output /tmp/fm-kernel-check \
  --audit-tool "$HOME/code/swift/dgen/scripts/audit-dgen-dylib.sh"

PYTHONDONTWRITEBYTECODE=1 python3 tools/fm-formant/validate_voice.py \
  --output /tmp/fm-voice-check \
  --audit-tool "$HOME/code/swift/dgen/scripts/audit-dgen-dylib.sh"

target/debug/instrument_probe "$PWD/tools/fm-formant/candidate/dsp.lisp" \
  --frames 48000 --gate-frames 24000 --min-peak 0.01 --min-rms 0.001 --json
```

The probe changes its working directory internally; use an **absolute DSP path**
if invoking the last command from an environment where that changes resolution.
The standalone Python harness is currently macOS-only. Its generated dry WAV is
a sequentially rendered chord demonstration, not a simultaneous-voice benchmark.

## Release boundaries

### Published lazy waveform selection (2026-09-08)

DGenLisp [v0.1.14](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.14)
is published, pinned in `content/dgenlisp.lock`, fetched, and used by the installed
FM Formant. The macOS archive SHA256 is
`da4d696db2e9c3c98871f2f04950a5de6d7def67e855a08dffc7f994a24b2162`.
The Linux pin is unchanged.

Gates skip the unselected sine/formant waveform calculation. They do not turn off
whole operators when output or noise levels are zero. Wave changes latch on retrigger.
Scalar leaf-table reads are supported inside gates. Sequential feedback fragments
now share one sample loop: the original experiment incorrectly introduced block
latency at gate boundaries. That compiler bug is fixed, not tolerated by a looser
signal comparison. Twenty targeted compiler tests pass.

Matched published-compiler medians, microseconds per 128 frames at 48 kHz:

| Case | Eager | Lazy |
|---|---:|---:|
| Eight formant operators | 586.33 | 618.69 |
| Sine FM | 612.74 | 185.92 |
| Four formant operators | 585.28 | 408.78 |
| Feedback FM | 614.59 | 190.59 |

All 96 preset/pitch equivalence cases pass (maximum sample error below 7.3e-7),
as does the mid-block retrigger comparison. Five generated programs pass fusion
checks. Host `instrument_probe` passes on the installed instrument and published
compiler. These are one-voice microbenchmarks, not polyphonic callback measurements.
All-formant patches remain expensive and are slightly slower with gates here.

`results/published-gates.json` records the corrected measurements. Earlier
`gates.json` and `gates-feedback-diagnostic.json` are historical evidence of the
failed experiment, not current performance claims. The benchmark harness now
sends a trigger before warming up, so latched waveform modes actually take effect.

### Audition UI

The left bank exposes oscillator A–D ratios and levels. Select a letter to edit
its waveform, amplitude envelope, frequency envelope, or FM inputs in the center.
Noise has a separate bank and independent amplitude/frequency envelopes. Performance
controls and global amplitude, motion, and keyboard pages sit on the right.
The waveform selector controls the lazy DSP branch; bank/page navigation only
changes the editor. Labels are vertically centered; the panel contains no tutorial
paragraphs. All 186 parameters remain reachable through 24 tested pages.

### Remaining implementation

The implementation epic is `eseq-9elc`; the DSP investigation `eseq-kb9y` remains
unfinished. Work is tracked in Beads, with the following concrete boundaries:

| Issue | Required before factory promotion |
|---|---|
| `eseq-9elc.1` | Complete-voice performance, reusable compiler/runtime improvements, real polyphonic measurement |
| `eseq-9elc.2` | Structured program assets, in-app motion editing, atomic revisions, persistence/undo/export |
| `eseq-9elc.3` | Expression/noise isolation, remaining DSP contracts, UI/patcher, listening and cross-platform validation |

Specific limitations must remain explicit: forward routes follow operator order;
feedback routes are always delayed; graph revisions are not immutable per-note
programs; operator mode is latched on retrigger, but other structural controls are
not fully revisioned. The performance controls have fixed roles rather than
editable mapping tables. A selected motion at amount 1 overrides base fields;
base operator level does not independently mute that animated level. The noise generator has separate
band seeds but synchronous equal-note voices can share identical streams. Its
resonator width specifies pole decay and has not been calibrated as exact digital
−6 dB bandwidth. Host voice allocation/stealing/mono lifecycle, program round trips,
extreme PM quality, and Linux behavior have not been certified.

There are no generated-C patches, hidden oversampling paths, or silent reductions
of PM depth or voice count. The candidate is an explicit development artifact,
not a workaround promoted into the factory library.

The opening Algorithm page uses Heat-style reactive `defwidget` diagrams. Parallel,
two two-operator stacks, and a four-operator chain
set all forward PM, feedback and carrier output sends as one undoable batch;
oscillator timbre, levels, envelopes and motion are preserved. Edited routes that
do not match a choice display Custom algorithm. The per-oscillator routing page
shows its incoming FM (accent) and one-sample feedback (gray); clicking a source
node opens that oscillator's routing page. Native parameter-lock batch commands
are used when steps are selected.
