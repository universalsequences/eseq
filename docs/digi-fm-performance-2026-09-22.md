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
support. Both sides use the then-pinned v0.1.25 compiler. The best prototype does
not require the compiler fix described below, subsequently released in v0.1.26.

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

The DGen change processes conjunctions by increasing predicate count and
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
comparisons are sample-identical. The fix and tests were subsequently published
in v0.1.26, as recorded below. Factory DSP remains unchanged.

Reproduction and raw results live under `.local/benchmarks/digi-fm-2026-09-22/`:
`switching-experiment.py`, `switching-final-validation.py`,
`compiler-gate-validation.py`, the `switching-*` result folders, host-probe JSON,
and `compiler-blowup-debug.sample.txt`. The final timing/state/audio report is
`switching-final-validation/results.json`. Tracking: eseq-c519.4 and dgen-8pj.

### Published compiler, 2026-09-23

[DGenLisp v0.1.26](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.26)
publishes the macOS arm64 fix from source commit
`e0751d26a280bfd0de4da79472f9a6e84f603a2e`. Eseq's macOS compiler pin is updated
and installed through `scripts/fetch_dgenlisp.sh`; the Linux pin and hermetic
Clang/lld stage are unchanged. The anonymous public download and installed
archive match SHA256
`f00cedfbdef7aa64a051a1b363621deed55a270612a5e40b6f0f63120e6fcc16`.

The release was built in a clean isolated checkout with Apple Swift 6.2.3 and
Xcode 26.2, stripped and ad-hoc signed. All 16 focused execution-gate tests pass.
Both the package and installed symlink pass scalar and grouped-parameter compile
checks, including the bundled binary audit, without resource-path overrides.
Four complete Digi FM sources produce byte-identical C to the previously
validated compiler. The formerly stalled source compiles in 3.06 seconds from
the package and 3.09 seconds through the installed symlink.

With the installed compiler, the production `instrument_probe` load/init path
passes for both factory Digi FM and `user:Experiments/Digi FM Fast Test` at
48 kHz / 512 frames, algorithm 6, Harmonics=5. Both render 8,192 frames with
finite audio/state and nonzero signal. The test instrument also switches to
algorithm 2 at frame 1,031 and back to 6 at frame 3,077. Peak/RMS are
0.05112/0.02088 for the factory instrument and 0.05105/0.02062 for the test
instrument. Fetching again confirms the installed distribution matches the pin.

This release fixes compiler analysis time; it does not itself change Digi FM's
audio-thread CPU cost or promote the experimental instrument into the factory
library. Release notes, packaged/installed checks and host-probe results are
under `.local/benchmarks/digi-fm-2026-09-22/release-v0.1.26/`; the fresh focused
test log is `compiler-release-tests.log` in its parent directory. Tracking:
eseq-c519.6.

## Further 30% reduction: remaining cost ceilings, 2026-09-23

The user auditioned `Digi FM Fast Test`, reported that it works well and feels
approximately twice as fast, and asked what another 30% reduction would take.
The target is about 220–238 µs per voice, relative to the measured 315–340 µs
Fast Test baseline below. These figures are single-voice native CPU time at
48 kHz / 512 frames, not a new measurement of the transport meter.

The installed source, SHA256
`2b68a9f9c8337e3f044e67eca96a22152a674d7fb78adbd7527a02fc0b755efd`,
was copied into isolated diagnostic variants. All use the pinned v0.1.26
compiler, algorithms 2 and 6, Harmonics 0 and 5, Type I selected, and seven
rotating-order repetitions. Compilation and Python are outside the native
timed region. Fourfold integration and live algorithm dispatch are retained.

| Diagnostic variant | Algorithm 2, µs | Algorithm 6, µs | CPU reduction vs Fast Test |
| --- | ---: | ---: | ---: |
| Unchanged Fast Test | 314.8–316.1 | 339.3–340.3 | — |
| Replace modulation accessors with bare parameters | 231.8–232.5 | 258.1–258.4 | 23.8–26.4% |
| Remove unselected Type II filter | 251.5–255.2 | 276.4–278.3 | 18.2–20.4% |
| Both changes together | 173.8–174.0 | 199.0–199.6 | 41.2–45.0% |
| Remove all filters | 220.1–221.7 | 246.3–247.8 | 27.0–30.4% |
| Replace harmonic waveforms with sine | 271.4–271.6 | 273.5–275.0 | 13.8–19.6% |

These are cost ceilings from feature-removing ablations, not shippable
optimizations. The combined result was measured directly; it is not the sum of
the individual percentages. Six static renders per variant (algorithms 2/6,
Harmonics 0/5/-5, no assigned modulation, Type I selected, including release)
showed maximum sample differences of 2.61e-8 for bare parameters, 1.12e-8 for
Type I only, and 2.89e-8 combined. They do not validate modulation assignment
changes, Type II, filter transitions, all algorithms or all presets.

The strongest first implementation target is compiler rate specialization for
unassigned modulation (`dgen-7tx`). `ModulationGateLoweringPass` already gates
the modulation arithmetic. Its inactive result still has audio-rate downstream
scheduling, including buffers and calculations that could otherwise be
block-invariant. A production solution must select the efficient unassigned
path at runtime while preserving parameter identities, smoothers, shared
state, and sample timing when assignments are added or removed. Removing
modulation accessors globally would violate that contract.

The other substantial target is the always-running pair of nonlinear filters.
`drift-filter-morph` deliberately advances both state histories. Sleeping the
inaudible filter can save work during a fixed selection, but waking it with
stale or reset state changes the existing transition. Warm-up/crossfade behavior
would need explicit design and qualification; it cannot be called bit-identical
to an always-running nonlinear filter. If exact current filter transitions are
required, keep both states running and seek the remaining saving in compiler
scheduling/control work instead.

This establishes credible headroom for a further 30% reduction, not a guarantee
for patches with many actively modulated parameters. No installed instrument,
factory DSP, compiler source or oversampling setting was changed. Reproduction:
`.local/benchmarks/digi-fm-2026-09-22/remaining-costs.py`; source snapshots, raw
timings and comparison metrics are in `remaining-costs/results.json` and its
`assets/` directory. Tracking: eseq-c519.7.

## Filter gating and compiler qualification, 2026-09-23

The user accepts freezing an unselected filter because filter-type plocks are
rare. `drift-filter-morph-gated` now provides that explicit policy alongside
the existing continuously running morph macro. Both filters run during the
existing smoothed crossfade. Contributions below 0.0001 (-80 dB) snap to zero,
so an exponential fade actually reaches the endpoint and the unused filter can
sleep. Its integrators resume from their retained state when selected again.
Filter-type changes consequently do **not** reproduce the old continuously
running filter histories, and may depend on callback boundaries while waking
or sleeping. Fixed selections retain the existing filter calculation.

The installed `user:Experiments/Digi FM Fast Test` uses the new macro. Its DSP
SHA256 is `979795a5f088b2db60703b9ec18ce8e8622281c7bc1e810fc3a095e65a8643d7`.
Factory instrument call sites and the compiler pin remain unchanged. Reload
the instrument/project to use the updated experimental DSP.

Native CPU time, one voice, 48 kHz / 512 frames, Harmonics=5, seven alternating
repetitions, compiler v0.1.26:

| Selected filter | Algorithm | Previous Fast Test, µs | Gated, µs | Reduction |
| --- | ---: | ---: | ---: | ---: |
| Type I | 2 | 316.8 | 255.8 | 19.3% |
| Type I | 6 | 344.4 | 283.3 | 17.7% |
| Type II | 2 | 317.1 | 294.7 | 7.1% |
| Type II | 6 | 345.5 | 324.6 | 6.1% |

This achieves part of the requested additional 30%, not the whole target.
For these Type-I cases another approximately 13–15% reduction from the gated
version would reach 30% relative to the previous Fast Test. These are isolated
DSP timings, not transport-meter measurements.

Validation against the saved pre-gating source:

- 180 fixed-selection comparisons: all eight algorithms at Harmonics -5/0/5,
  six factory presets, both filter types, and three pitches. Maximum absolute
  sample difference 7.45e-8.
- 54 additional comparisons cover all nine installed Fast Test presets at
  three notes and both types, with changing modulation inputs. Maximum
  difference 7.45e-8; maximum normalized RMS error 1.53e-6. Host LFO programs
  are not recreated by this native harness.
- All 56 ordered algorithm transitions, with retriggers and irregular process
  sizes, pass for both filters (112 comparisons; maximum difference 2.42e-8).
- Audio-rate harmonic modulation, release to silence, and repeated filter
  switches at zero/maximum resonance remain finite. Filter-switch output is
  checked for bounded signal rather than equality to the old running state.
- `instrument_probe` passes the real compile/load/init path for the installed
  name with Type II, and for the candidate source with Type I→II→I changes at
  frames 4099/10003. Both have finite state/audio and nonzero output.

Reproduction: `tools/digi-fm/validate_filter_gating.py --baseline <saved-dsp>
--candidate <gated-dsp> --output <results-dir>`. Both DSP files must be expanded
and have their `spectra.json` asset beside them. Source snapshots, timings,
supplementary preset/switch tests and results are in
`.local/benchmarks/digi-fm-2026-09-23/filter-gating/`.

Two compiler prototypes were investigated and **rejected**, then removed from
the dgen working tree:

1. Conditional static control expressions duplicated only pure arithmetic,
   selecting a block-level path when modulation was unassigned. Shared DSP
   state remained single-owned. It saved only about 0–2% on Fast Test. More
   importantly, it failed numerical sound comparisons in eight drum engines:
   909 Open Hat, Digi Cymbal, Digi Hat, Membrane Snare, Modal Kick, Modal Snare,
   Orbit Tom 66, and Virus B BassDrum 23. Generated code changes between scalar
   and SIMD math; in Membrane Snare the first difference is floating-point
   roundoff, then grows through the nonlinear model. This is evidence against
   shipping, not proof that every waveform difference is audible.
2. Direct reads through block-uniform buffer selections preserved the existing
   arithmetic schedule. Fast Test comparisons passed, including all eight
   algorithms/both filters, but timings were within measurement noise. Only
   eight selections qualified there; adding this machinery does not currently
   justify its complexity. A focused native test also covered changing two
   modulation assignments around shared history at block sizes 1, 7 and 64.

`tools/digi-fm/compare_factory_compilers.py` snapshots all 40 factory instruments,
assets and recursively expanded macros, plus all 296 factory presets. Each
instrument runs in an isolated process through the native production ABI at
48 kHz / 512 frames. Default plus every preset is rendered at three notes for
1.5 seconds, with release and retrigger: 1,008 baseline recordings. The first
compiler prototype compiled all 40 instruments, but 124 recordings exceeded
the comparison limits (peak error 2e-5 or normalized RMS error 1e-4), across the
eight engines above. The checker also now records one audio-rate modulation /
assignment-change case per instrument using irregular callback lengths, for
1,048 baseline recordings in total. It checks DSP directly; host audio effects
and host preset LFO programs are outside this coverage, and unused stored
preset fields are explicitly recorded.

After removing the prototypes, both local compiler build profiles were rebuilt
from the clean tracked source. A final control comparison of the restored
release compiler against the installed v0.1.26 distribution passed all 40
instruments and all 1,048 recordings **bit-for-bit**, with zero compilation or
signal failures (`factory/restored-control.json`). This validates the reference
comparison and the compiler restoration; it is not an optimization result.
Factory Digi Drift also passes `instrument_probe` with the expanded shared
macro module after the new, unused gated macro definition was added.

The failed compiler source/patches and numerical results remain under
`.local/benchmarks/digi-fm-2026-09-23/compiler/` and `factory/`. Disposable native
build caches from rejected/older Digi FM experiments were removed after the
disk filled; their source snapshots and result data remain. No compiler release
or vendor update was made. Assignment-aware scheduling remains open as
`dgen-7tx`; its next design must preserve the existing arithmetic precision and
SIMD behavior as well as DSP-state ownership. Merely hoisting calculations or
relaxing comparison tolerances is not an acceptable completion.

## Factory promotion — 2026-09-23

At the user's request, the approved Digi FM Fast Test now supplies the factory
`Synths/Digi FM` implementation. `tools/digi-fm/build.py` generates the cumulative
8192 × 112 harmonic bank, eight gated literal-algorithm cores and the gated
Drift filter call directly. This replaces the experimental source-rewriting
pipeline with maintained generation. Fourfold integration, shared phase and
feedback histories, reconstruction filtering and live algorithm changes are
unchanged from the approved experiment.

The expanded factory DSP has exactly the same 6,390 Lisp tokens as the installed
Fast Test after removing comments/whitespace. The generated bank is byte-for-byte
identical: SHA256
`95050052aff4e269971efd001169280cdc75ff02094b9a796b384f192a387e91`.
Generation is idempotent for DSP, table and parameter defaults. The factory
identity, UI, all parameter declarations and the six-preset bank are unchanged.
Existing projects and the separate Fast Test instrument were not edited.

Validation with the pinned macOS v0.1.26 compiler:

- 270 audio comparisons against the approved Fast Test are **bit-for-bit
  identical**, including all eight algorithms with both filter types at
  44.1/48 kHz; all nine audition-bank presets at three notes and both filter
  types; every ordered algorithm transition; filter changes with/without
  retriggers; and audio-rate Harmonics modulation with irregular callbacks.
- `python3 tools/digi-fm/validate.py` passes its independent routing/table
  reference, 16/128-frame parity, envelope/retrigger/hold/release, output gain,
  high-rate spectral reference and unchanged Digi Drift checks. Its existing
  eight-voice 128-frame benchmark measured 0.587 ms versus a 2.667 ms deadline;
  this excludes host graph scheduling.
- `performance.py` now stages each revision's own adjacent assets. Its 66 audio
  checks pass at 32/128/512 frames. A single paired timing run checks the harness
  and gives similar Fast Test/factory timings; it is not a new performance claim.
- Production `instrument_probe` loads all six factory presets with finite,
  audible output. A separate 16,384-frame run changes algorithm at frame 1,031
  and filter type at 4,099/10,003, also without non-finite samples or state.

Evidence is in `.local/benchmarks/digi-fm-2026-09-23/factory-promotion/`,
`factory-promotion-validation.log` and `factory-promotion-host.log`. No compiler
source or pin changed. Linux was not exercised during this promotion.

The sound reference for this promotion is the user-approved Fast Test. As
documented above, table interpolation is not bit-identical to the original
additive synth in feedback-sensitive patches, and the gated filter freezes
inactive topology state. This promotion adds no further numerical changes.
