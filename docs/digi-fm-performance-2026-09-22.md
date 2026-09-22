# Digi FM DSP investigation, 2026-09-22

The instrument kernel is the primary bottleneck in the supplied main-audio-thread
profile. Of 3.14 seconds under `bind_and_run_live`, 2.99 seconds (95.2%) are under
the custom instrument wrapper. This is a proportion of the displayed node work,
not of the entire application or all audio workers. The nested `dgen_process_v1`
and `t…_g` entries must not be added again to the wrapper's inclusive time.
The generated `t…_g` names are scratch arrays, not separate DSP algorithms.

The prominent 471 ms in `sinf` is only 15.8% of the wrapper's 2.99 seconds.
Optimizing sine calls alone cannot explain or remove the entire gap. Reverb,
delay, mixing and host modulation are small in this profile.

The saved `tryingtodigifm` project contains three factory Digi FM tracks plus two
empty MIDI tracks. The instruments share the same source and the host's
12-voice engine pool; each saved track has a six-voice limit. This confirms the
capacity model, not the number of voices active at the moment of the profile.
Two tracks have Harmonics=0. The third has Harmonics=0.069999695 and a modulation
assignment. Releases are approximately 4.06, 1.60 and 6.07 seconds.

## Controlled measurements

Machine: Apple M1 Max. Compiler: pinned macOS DGenLisp v0.1.25. Source at
`1db3523713de218db5764887fe3ed54721b8f37f`, SHA-256
`1b741e06f7a231a8a1da4b019cfbaa744946cc49deacad65ab78b701e74800c8`.
Compiler executable SHA-256:
`d37b6cb7d097616f80c7cf7e34b924510be07ac04948f5897da52f522102e8b8`.

The native C driver measures process CPU time inside repeated production-ABI
calls, with allocation, compilation and Python outside the timed interval.
Pitch is 220 Hz, velocity 0.8, with two retriggers per second. These are one-voice
measurements without the host graph; they do not reproduce the project's
parallel callback wall time or the Operator comparison. The user's app remained
open, so the raw repetitions are retained and no quiet-machine claim is made.

Initial five-repeat medians at 48 kHz / 512 frames, Harmonics=0:

| Experimental source | Microseconds per voice |
| --- | ---: |
| Complete unchanged Digi FM | 666.5 |
| Replace harmonic waveform macro with sine, retain the rest | 368.3 |
| Original FM plus decimator, omit output/filter/amp path | 568.2 |
| Sine FM plus decimator, omit output/filter/amp path | 271.9 |

The sine-only substitution removes a feature and is an ablation, not a proposed
production replacement. At zero Harmonics, however, all additional partial
weights are already zero. The approximately 45% difference exposes work the
implementation should be able to avoid for that setting. Nonzero Harmonics
costs essentially the same as zero in the current implementation.

A later seven-repeat unchanged-source control measured 40.85 µs at 32 frames,
162.90 µs at 128 frames, and 661.32 µs at 512 frames. Each is about 6.1–6.2% of
one core's audio-time budget per voice. Twelve serial voices would consume
roughly 7.94 ms out of the 10.67 ms deadline at 512 frames. The app distributes
work across helpers, so this is not a prediction of its DSP meter. Increasing
the buffer does little for work that scales directly with the number of samples.

## Implementation findings and rejected experiment

Digi FM evaluates five waveform expressions per internal step (A is evaluated
in two dependency positions), with four internal steps per host sample. Each
waveform supports a 16-partial additive recurrence. Both nonlinear Drift filters
also run continuously so their smoothed topology blend has live state.

The baseline generated C contains 117 `for` loops and 1,498 scratch arrays.
Those counts identify candidates for investigation, not measured attribution.
Forcing scalar execution was worse: approximately 1,385 µs per voice.
Increasing scalar coalescing to 4,096 nodes was also worse, approximately
1,070 µs. Existing SIMD must be preserved where it helps.

A normal `gswitch` around the waveform did not skip its producers and cost
approximately 716 µs. Explicit `block-gate` around the pure harmonic correction
did skip work: one interleaved zero-Harmonics comparison measured
695.1 → 418.7 µs, a 39.8% reduction. That experiment was **not retained**:
regrouping the sum failed waveform comparison in algorithm 1 at Harmonics=2.5.

A second candidate preserved the original sum order and selected an
unconditional fundamental when the gated full spectrum was zero. It still
failed algorithm 5 / Harmonics=6 / Feedback=0.15: peak difference 0.026935,
normalized RMS difference 9.70% over 0.12 seconds. Early differences were around
1e-11 and grew later; with feedback disabled the maximum difference was
1.40e-7. This is consistent with rounding amplified by sensitive feedback,
but is not sufficient evidence to declare the sonic change harmless. No
comparison threshold was relaxed to accept the optimization.

Both candidate changes were removed from the factory DSP and generator.
The remaining work is tracked in `eseq-c519`: investigate the arithmetic and
scheduling changes, validate sensitive feedback adequately, and optimize the
waveform path with that evidence. A measured table-based spectral renderer is
another candidate already anticipated by the original design. Oversampling,
filter behavior and feedback timing should not be changed merely to improve
the benchmark.

## Reproduction and validation

`tools/digi-fm/performance.py` retains the comparison and timing harness. Save
the original source before an experiment, then run it with `--baseline-source`,
`--candidate-source` and `--output`. Both sources use the same current shared
macros and compiler. The harness checks all eight algorithms at six harmonic
settings, all six presets, and sample-timed bipolar modulation, retriggers and
release through regular and irregular process partitions at 32/128/512 frames.

The unchanged-source control passed all 66 comparisons with zero sample error.
The rejected candidate reproduces the failure above. This validates the harness
and establishes the baseline; it does not certify an optimization.
The existing `instrument_probe` also passed through the production host path at
48 kHz / 512 frames: peak 0.04628, RMS 0.02282, no non-finite samples or state.
No full Rust suite was needed or run; production DSP and Rust are unchanged.

Raw data, generated sources, candidate, logs and compiler artifacts are indexed
under `.local/benchmarks/digi-fm-2026-09-22/`, including `initial-results.json`,
`gated.json`, `baseline-verified/results.json`, `rejected-candidate.log`, and
`host-probe.json`. The separately captured live `sample` trace was mostly idle
and was not used to estimate playing DSP cost. No live project CPU reduction
is claimed, and nothing was committed or pushed.

## Harmonic lookup-table experiment

The user's later requirement supersedes the initial willingness to trade sound
for speed: preserve the existing project sounds and spectral recipe. The
following experiments retain all four FM substeps, feedback timing, routing,
envelopes, filters and output scaling. They are isolated prototypes; the factory
instrument and generator are unchanged.

Elektron's designer Ess explicitly confirms interpolated oscillator lookup
tables, a 32-bit 96 kHz engine with additional oversampling, and DSP routines
implemented in assembly in [this 2018 explanation](https://www.elektronauts.com/t/sound-quality-characteristics-digitone-vs-volca-fm/53072/34).
Elektron's [Harmonics description](https://support.elektron.se/support/solutions/articles/43000566560-harmonics)
describes additive spectra and interpolation between them. These sources make
precomputed harmonic waveforms a plausible implementation, but do not reveal
exactly how Elektron stores or evaluates Harmonics. Our table layout is our own.

The prototype generates cumulative sums of the **existing** seven spectral
anchors at each partial count from 1 through 16: 112 periodic waveforms. At
runtime it interpolates phase and Harmonics within two adjacent partial-count
tables, then interpolates those using the existing frequency-dependent fade.
Thus it retains the mathematical spectral definition, including high-note
partial attenuation, rather than simply playing a full-band waveform at every
pitch. Finite table interpolation and floating-point reassociation prevent a
claim of bit-identical output, especially inside feedback.

Five alternating native timing repetitions per condition at 48 kHz / 512 frames:

| Table samples per waveform | Baseline range (µs) | Candidate range (µs) | Speedup |
| --- | ---: | ---: | ---: |
| 2,048 | 660.5–664.4 | 503.7–506.6 | 1.307–1.317× |
| 8,192 | 664.9–670.6 | 509.0–515.6 | 1.299–1.315× |

Ranges span Harmonics 0, 0.07, 5 and -5. The 8,192 run overlapped some separate
comparison rendering, and the application remained open; neither is a
quiet-machine benchmark. Both support approximately 24% lower isolated kernel
CPU cost, not the requested eightfold gain and not a measured app-meter change.
A separate gate that skipped the upper table read at integer partial limits was
worse (about 528 µs), and changed some rounding-sensitive results; it is rejected.

The wave primitive passed 48 combinations of frequency and morph, including
negative and multi-cycle phases. Maximum absolute error against the independent
additive formula was 4.67e-5 at 2,048 samples and 7.97e-6 at 8,192. Each full
candidate also passed 96 finite/bounded renders across all eight algorithms,
six bipolar harmonic settings, low/high notes and ordinary/extreme feedback.
For the 8,192 candidate, sample-timed harmonic modulation rendered identically
through regular and irregular process partitions, and `instrument_probe`
passed the real host compile/load/init path (Harmonics=5, peak 0.04320,
RMS 0.01945, no non-finite audio or state).

Full-patch comparisons cover all six factory presets and the three saved
`tryingtodigifm` track settings at 110, 440 and 1,760 Hz. For the 8,192 table,
the nine static project-setting comparisons have normalized RMS error between
2.49e-7 and 6.08e-7. Three additional synthetic bipolar modulation sweeps use
track 4's real hidden Harmonics enable and depth=7.09: error is 1.20e-6 to
1.46e-5 (approximately -118 to -97 dB relative RMS). This does **not** replay
the neural sequencer or all project automation.

Feedback-sensitive sounds remain a qualification problem. The factory Folded
Lead preset has normalized waveform RMS differences of 4.51–8.83% with the
8,192 table, despite the close wave primitive. A larger table did not reliably
remove this divergence. These values are numerical differences, not percentages
of sound lost. Spectral diagnostics and matched-level WAVs are retained; there
has been no listening verdict, so sonic identity is not certified. Raw table
storage is also material: 3.5 MiB per voice for the 8,192×112 float table, or
42 MiB for twelve voice states if each holds its own copy.

Reproduction scripts and outputs are under
`.local/benchmarks/digi-fm-2026-09-22/`: `table-experiment.py` (optionally
`--samples 8192`), `table-comparison.py table-8192-4x`, and the `table-4x` /
`table-8192-4x` directories. Each output directory contains the source snapshot,
assets, timings, comparisons and original/table WAV pairs. The parent task
`eseq-c519` remains open for production optimization and sound qualification.

## Reaching a twofold reduction

The next investigation targets half the original kernel cost, approximately
333 µs per voice at 48 kHz / 512 frames. The strongest measured additional
lever is specializing the FM topology. The general graph contains dynamic
routing selectors and two positions for operator A; it does not evaluate eight
complete synth engines. Supplying a constant algorithm lets the compiler prune
unused paths and simplify the dependency graph.

These diagnostic variants replace the internal `alg` signal with a literal.
They retain the same 4× integration, Harmonics, decimation, both nonlinear
filters, envelopes and modulation, but **cannot switch algorithms**. They
establish the cost of a specialized path, not a finished dispatch implementation.

Five alternating repetitions per condition, with the same native timer:

| Configuration | Algorithm 2 (µs) | Algorithm 6 (µs) |
| --- | ---: | ---: |
| Original, in the final paired run | 653.4–654.4 | 656.5–661.3 |
| General table renderer | about 505–510 | about 504–510 |
| Table + fixed topology, earlier paired run | 316.6–317.0 | 334.4–335.6 |
| Table + fixed topology + original sine for B2 | 291.9–292.1 | 323.1–324.0 |

Ranges span Harmonics 0, 0.07, 5 and -5. B2 is structurally a sine in all
algorithms; the final variant uses the original `sin(twopi * phase)` for it
instead of the generic harmonic-table machinery. Combined speedups versus
the paired original are **2.237–2.242× for algorithm 2** and **2.030–2.041× for
algorithm 6**, the two algorithms used in `tryingtodigifm`. Algorithm 3 was the
slowest of the eight fixed-topology table variants at Harmonics=5; adding the
same B2 specialization gave 660.9 → 329.1 µs (2.008×) in that condition.
This is not a universal twofold guarantee across every setting or a measurement
of the complete application's DSP meter.

The fixed-topology table variants were compared with the general table renderer
for all eight algorithms at four harmonic settings over 0.4 seconds, including
note-off. Seven algorithms had maximum normalized RMS differences below 1.85e-6;
algorithm 5 again exhibited feedback sensitivity (up to 2.76%). The combined
B2/topology variants for algorithms 2, 3 and 6 had maximum normalized RMS
differences below 5.18e-7 against the general table renderer in those same
conditions. This isolates the additional change; it does not erase the earlier
qualification needed for table versus original additive rendering.

Other ablations help rank the remaining work. In the general table version,
removing all filters measured about 425–428 µs, leaving only Type I about
453–457 µs, and replacing all `(mod param)` reads with bare parameters about
436–437 µs. None is a production candidate: they remove sound paths, transition
state or modulation. Their savings must not be added to one another as if
independent. The routing specialization reaches the target while retaining
those features, so it is the preferred next implementation direction.

Production work requires efficient precompiled paths for all eight algorithms,
selection of the active path, and a deliberate shared voice-state contract.
The existing fade-to-zero transition, sample of the routing change, phase
continuity and feedback reset must be retained. Simply rebuilding or swapping
an independent instrument with a different memory layout on a knob change is
not an implementation of that contract. Full algorithm-transition, modulation,
irregular-partition and host-project validation remains necessary.

A complementary `block-gate` experiment around sine/table alternatives spent
over three minutes in the pinned compiler without completing and was stopped.
The one-second process sample places the work inside `CompilationPipeline`,
but does not resolve the responsible pass. This is tracked in `eseq-c519.3`;
it needs investigation before using that gate pattern for production dispatch.
The separate full-spectrum/additive hybrid variant was never reached, so no
performance result is claimed for it.

Reproduction and raw output: `twofold-experiment.py --skip-gated`,
`algorithm-specialization.py`, `algorithm-specialization.py --b2-sine`, and
their correspondingly named directories under the same local benchmark root.
Factory DSP, its generator and the compiler are unchanged. Investigation
`eseq-c519.2` records these results; `eseq-c519` tracks the production work.

Both combined algorithm-2 and algorithm-6 prototypes also passed the production
`instrument_probe` compile/load/init path at 48 kHz / 512 frames, Harmonics=5,
8,192 frames. Peaks were 0.04320 / 0.05105 and RMS 0.01945 / 0.02088,
with no non-finite samples or state. These remain single-note host checks,
not a test of live algorithm dispatch or full-project CPU.

## Existing fast-sine macro

The checked-in `content/defmacros/fast-sine/macro.lisp` is a wrapped phase-domain,
degree-13 polynomial using Estrin evaluation. Its own comment notes that
feedback-loop performance previously reached parity with libm. The current
experiment uses that macro unchanged; it does not invent a cheaper approximation.

Seven rotating-order repetitions per condition at 48 kHz / 512 frames:

| Change | Existing calculation (µs) | With fast-sine (µs) |
| --- | ---: | ---: |
| Original additive instrument, sine replacement only | 658.2–661.5 | 714.4–716.6 |
| Original additive instrument, sine and cosine replacement | 658.2–661.5 | 705.1–706.6 |
| Specialized table algorithm 2, B2 sine replacement | 293.7–294.5 | 292.7–293.1 |
| Specialized table algorithm 6, B2 sine replacement | 324.4–325.2 | 324.5–325.7 |

Ranges cover Harmonics 0 and 5. Cosine uses `fast-sine(phase + 0.25)` in its
separate variant. The original additive engine becomes approximately 7–9%
more expensive; in the specialized table paths the differences are under
0.5%, comparable to run variation. There is no useful measured CPU gain here.
The polynomial trades a library call for explicit arithmetic and range wrapping;
its name alone does not imply a win in this generated feedback graph.

The primitive was checked against an independent sine reference over 49,152
phase samples spanning three cycles near 0, -32 and +32; maximum absolute error
was 1.85e-7. Selected complete patch comparisons at Harmonics 0, 5 and -5
remained close, with maximum normalized RMS difference 1.124e-6. These are limited numerical checks,
not broad sonic certification, and no macro replacement is proposed for shipping.
Raw measurements and the reproducible script are in `fast-sine/results.json`
and `fast-sine-experiment.py` under the local benchmark root. Production DSP
and the fast-sine macro remain unchanged.

## Live algorithm switching and compiler investigation

The next isolated prototype retains all eight algorithms and their live
selection. It specializes the four oversampled FM steps and X/Y mixer together
inside one `block-gate` per algorithm. Phases, feedback histories, envelopes,
decimator histories and downstream filters retain shared ownership. The
existing routing fade/reset code is unchanged. B2 uses the original sine
operation, and the other waveforms use the 8,192-sample cumulative tables.

`sample` already wraps normalized phase in the compiler. Removing the extra
wrap immediately before it saved another 5–9 microseconds in most cases. This
change was not perfectly bit-identical: 64 comparisons against the otherwise
identical switchable prototype, including high pitch and feedback=2, had maximum
sample error 2.05e-8 and maximum NRMSE 1.73e-7.

### Final quiet native timing

M1 Max, 48 kHz, 512 frames, one voice; seven rotating repetitions per condition,
with compilation and other experiments finished before measurement. These are
complete native instrument calls, including filters, envelopes and modulation
support. Both sides use the pinned v0.1.25 compiler. The best prototype does not
require the unpublished compiler fix described below.

| Algorithm | Original, µs | Switchable table prototype, µs | Speedup |
| --- | ---: | ---: | ---: |
| 1 | 677.0 | 320.7 | 2.11× |
| 2 | 667.5 | 310.4 | 2.15× |
| 3 | 670.2 | 342.2 | 1.96× |
| 4 | 671.6 | 334.6 | 2.01× |
| 5 | 671.4 | 331.0 | 2.03× |
| 6 | 668.8 | 335.8 | 1.99× |
| 7 | 669.9 | 318.5 | 2.10× |
| 8 | 665.2 | 304.5 | 2.18× |

The table uses Harmonics=5. Across Harmonics=0, 0.07, 5 and -5, algorithm 2
measured 2.140–2.151× and algorithm 6 measured 1.990–2.004×. This supports an
approximately twofold single-voice reduction with live switching; it does not
establish a strict 2× floor for every algorithm or a measured whole-project
transport-meter improvement.

Several alternatives lost performance and remain rejected experiments:

- Dispatching each oversampled step separately cost about 507–584 µs.
- Moving the decimator inside each algorithm region cost about 367–414 µs.
- Adding nested sine/table execution gates cost about 391–454 µs for algorithm
  6; sharing the waveform predicates improved that version but still lost to
  the ungated table path. Successful compilation does not imply a runtime win.

### Switching and audio checks

All 56 directed algorithm transitions were exercised, both with held notes and
with retriggers. Each render alternates a pair of algorithms in both directions
and continues through note-off. Normal versus irregular block partitions were
sample-identical. Additional rapid changes exercise a new request before the
previous fade has finished, changes inside blocks, retriggers and release.

Diagnostic outputs compared algorithm selection, routing gain, switch pulses
and all four phase histories against the original additive instrument. All 18
instrument/phase-reset/retrigger combinations matched those controls exactly;
feedback and decimator input state were zero on the same reset samples. The
normal and irregular partitions also matched exactly, including audio.

Sound identity remains a separate qualification. Against the general table+B2
sine reference, seven algorithms had static NRMSE below 2.48e-6, while algorithm
5 reached 0.480% in the tested static renders and transitions involving it
reached 3.69%. These are waveform differences in feedback-sensitive cases, not
perceptual scores. The existing table-versus-additive Folded Lead caveat still
applies; no listening verdict is claimed.

For the saved `tryingtodigifm` static settings at 110, 440 and 1,760 Hz, tracks 2
and 3 stayed below 2.78e-7 NRMSE against the original. Track 4 ranged from 9.36e-6
to 0.002282 NRMSE (0.228% at the highest tested pitch); maximum sample difference
was 2.93e-5. These renders include note-off but do not replay the neural
sequencer or its live modulation. A separate bipolar Harmonics modulation
render passed exact regular/irregular partition checks.

The production host `instrument_probe` path also loaded the best candidate and
its tensor asset successfully: algorithm 2/Harmonics=0 and algorithm
6/Harmonics=5 with two mid-render algorithm changes produced finite audio/state
and passed the signal thresholds. The latter rendered 8,192 frames, peak
0.05105 and RMS 0.01873. The 3.5 MiB table-per-voice storage qualification remains.

### Compiler fix

Sampling the locally built debug compiler confirmed the earlier stall was in
`ExecutionGatePass.prepare`, specifically `ExecutionDemand.include` repeatedly
sorting growing conjunction sets. A depth-first worklist explored narrow paths
through feedback before pending broader demands could subsume them.

The local DGen change processes conjunctions by increasing predicate count and
canonicalizes demands once after convergence. Propagation only preserves or
adds predicates, so this order processes every possible broader term first.
There is no cap, dropped condition or change to execution semantics. It avoids
transient combinatorial growth; it does not claim a polynomial bound when the
final minimal demand expression itself is exponential.

The previously stalled full complementary-wave patch compiles in **3.270 s**
with the local release compiler, versus the earlier run stopped after 182 s.
Sixteen focused Swift tests pass, including new shared-history growth and
Boolean reachability regressions. Original, table and eight-algorithm Digi FM
sources generate byte-identical C before/after the compiler fix, and 75 audio
comparisons are sample-identical. The fix and tests are local in
`~/code/swift/dgen`; no compiler release, eseq pin change or factory DSP change
has been made.

Reproduction and raw results live under `.local/benchmarks/digi-fm-2026-09-22/`:
`switching-experiment.py`, `switching-final-validation.py`,
`compiler-gate-validation.py`, the `switching-*` result folders, host-probe JSON,
and `compiler-blowup-debug.sample.txt`. The final timing/state/audio report is
`switching-final-validation/results.json`. Tracking: eseq-c519.4 and dgen-8pj.
