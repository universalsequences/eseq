# mow B2 performance investigation — 2026-09-13

Changing the first three kick rack slots from twelve voices to mono reduced
measured process CPU from approximately **229% to 148%**, and the transport
monitor from **44% to 29%**. That is about **35% less process CPU** in the actual
running app, with four workers. The fourth kick slot was already mono.

The subsequent DSP pass reduced warmed transport CPU from **29.75% to 26.13%**
at matching voice counts, another **12.2% reduction**. It makes the Grampian
spring in Space Echo cheaper while producing sample-identical tested audio.
The requested approximately 20% transport target has not been reached.

The saved B2 scene had these slot limits:

| Rack | Instrument | Voice limit before | After |
| --- | --- | ---: | ---: |
| Rack Modal Kick | Modal Kick | 12 | 1 |
| Rack Modal Kick | Break Kick 53 | 12 | 1 |
| Rack Modal Kick | DOOM Kick | 12 | 1 |
| Rack Modal Kick | 808 Kick | 1 | 1 |
| Rack PM Hi-Hat | PM Hi-Hat | 12 | 12 |

Both parent rack tracks had `polyphonic: false`, but rack playback uses each
slot's `max_polyphony`. The current slot controls showed poly, which the user
confirmed before changing the three kick slots. No allocation bug was established.
Mono reuses one voice, so overlapping pitches and release tails can sound
different. The reduction is a workload change, not a faster implementation of
the same polyphonic sound.

The initial fresh app and a subsequent fresh developer-app run were both
sampled after several bars of playback. The latter reproduced the reported
44% transport load and rendered twelve voices in each of the first three kick
slots. The retained custom-instrument release window is twenty seconds, so
measuring immediately after launch or immediately after reducing polyphony
would give misleading results.

The live comparison uses the same `metal_seq` process (PID 89016), with
`TINYSEQ_LOG_VOICE_COUNTS=1`. A read-only observer sampled cumulative process
CPU time using `ps` every two seconds and retained the corresponding voice
logs. Analysis selects intervals where all three kick engines have the expected
enabled count and discards the first three intervals of each state. No build,
kernel timing run, or sampling profiler ran during this comparison. This is
one live before/after observation, not a randomized repeated benchmark. The
unchanged hi-hat naturally fluctuated between eleven and twelve enabled voices;
the other instrument voice counts were stable.

A ten-second `sample` profile at a two-millisecond interval attributed these
shares of samples inside graph DSP jobs, including each kernel's math callees:

| DSP component | Share before changing polyphony |
| --- | ---: |
| Modal Kick | 33.3% |
| PM Hi-Hat | 14.5% |
| Break Kick 53 | 8.6% |
| DOOM Kick | 6.7% |
| Filter Table effects, combined | 5.3% |
| Gate/pitch processing | 4.4% |
| Space Echo | 4.3% |
| Digi Cymbal | 4.2% |
| Digi Drift | 3.1% |

These are sampled DSP-work shares, not whole-process CPU percentages or
critical-path timings. They make the kick layers and hi-hat the first places
to investigate, ahead of the effects, for this original polyphonic workload.

The headless experiment was also run with eight warmup bars and sixteen
measured bars, plus a separate 45-second warmup and 45-second measurement. It
did not reproduce the workload: the project's `demos.graph-variable-reset`
import publishes a graph through the UI authoring runtime, which the harness
does not initialize. Headless rack engines stayed at one rendered voice while
the full app reached twelve. Its roughly 94–96% CPU results are therefore
**not valid estimates for the original live project**. Proper shared authoring
initialization and export parity are tracked in `eseq-tdjl`.

An isolated Modal Kick DSP experiment remains outside production source. It
schedules pure modal coefficient math on exact sample-by-sample control changes
using `event-hold`, with final latches restoring continuous coefficients. It
keeps all modes, resonator state updates, and control timing. Seven alternating
native repetitions at 48 kHz and 512 frames measured median per-voice kernel
time of 435.7 µs before and 257.1 µs after (41% lower). The kernel was compiled
with capacity twelve; each timing call processes one voice. This is not a
41% whole-project improvement.

The candidate passed the existing Modal Kick preset/drive/tuning/retrigger/
44.1–96 kHz/partition checks. Forty-eight preset/pitch waveform comparisons
with contact noise disabled had maximum normalized RMS difference 0.001992;
this measures sample error, not perceived similarity. Twenty-four tests
covered individual and combined continuously changing modulation, adjacent
updates, and steps. Variable process partitions and voice index eleven matched
voice zero at fixed partitions exactly. Generated-C fusion audits were clean.
Worst-case modulation timing, final source cleanup, production host probing,
and full-project benefit remain to be established before shipping; tracked in
`eseq-sfdp`. This instrument candidate is not part of the production change below.

The initial harness change adds voice-workload diagnostics to the opt-in
`audio_experiment` result. Validation was a successful release build with
`--features audio-experiments --bin audio_experiment` and an eight-bar-warmup,
sixteen-bar-measurement run that exercised the added fields. No hot-path clocks
or new voice counters were added; the harness reads existing counters.

## DSP pass after the mono correction

The warmed live app was captured with four helper workers using the new opt-in
`TINYSEQ_AUDIOGRAPH_PROFILE` recorder. After another thirty seconds of warmup,
forty 512-frame graph slices were sampled over about twenty seconds. The median
summed kernel time was **9.727 ms**, while median graph wall time was **3.084 ms**.
Those measures differ because the callback and helpers process jobs concurrently.
The longest dependency chain by summed kernel duration was only 1.437 ms;
limited worker availability and dispatch gaps also affect completion.

In every capture, the actual last-completion chain ran through:

```text
last PM Hi-Hat voice -> causal FilterTable -> Space Echo on bus B -> output
```

Space Echo took a median **802 µs** per slice, after most parallel work had
finished. The causal FilterTable on that chain took 299 µs. The Modal Kick's
474 µs job was more expensive individually than a hi-hat voice, but did not
appear on the last-completion chain. This distinguishes an expensive kernel
from a serial dependency that determines callback completion.

### Grampian spring change

`effects/spring/grampian.rs` now advances the forward and return dispersion
cascades together. Both fractional-delay reads are strictly causal, including
all interpolation taps, so the current return-delay write can follow the
paired calculation. Each section stores the two legs' states together. Ordinary
Rust operations then compile into SIMD on this Apple Silicon machine, without
architecture-specific intrinsics or approximate math.

Every propagation path, allpass section, coefficient, interpolation operation,
feedback update, and pickup mix is retained. The internal state layout changes;
project parameters and saved project data do not change.

Eight alternating measured repetitions, following two warmup repetitions, gave
median isolated Grampian cost **738.7 µs -> 366.3 µs per 512 samples at 48 kHz**,
a **50.4% reduction**. This is the spring kernel's cost, not a whole-project
percentage. Timing ran after the user closed playback and before test builds.

Validation:

- Original and changed kernels produced exactly equal sample pairs at 8,
  44.1, 48, 96, and 192 kHz, each at tensions 0, 0.5, and 1.
- Four continuously automated cases compared every output and the complete
  final DSP state, accounting for its layout. They cover normal parameters,
  minimum delay lengths, uneven section counts up to the 128-section limit,
  and zero-section cascades. All comparisons were exact.
- Two new regressions prove paired sections match independent scalar legs and
  delay reads do not depend on the current frame's write.
- All fifteen focused library tests passed, including production Space Echo
  automation, stereo/partition invariance, type crossfades, late packets,
  decay and sleep/wake behavior:

  ```sh
  cargo nextest run --release -p sequencer --lib -E 'test(/grampian_/)'
  ```

The first test build failed because the disk was full, before running tests.
Stale debug incremental caches older than two days were removed and the
library-only retry passed. No full package suite was run. Native performance
measurements are for Apple Silicon; Linux performance was not measured.

### Live result with four workers

The changed app was warmed for about a minute, then observed for sixty seconds
with no trace requests, builds, or kernel benchmarks running. The original app's
last thirty two-second monitor readings provide the baseline. Its trace had
already finished before that interval. Both builds used the same experiment
feature, with the recorder inactive during the CPU observations.

| Measurement | Before | After |
| --- | ---: | ---: |
| Mean transport CPU, all readings | 29.75% | 26.09% |
| Mean transport CPU, twelve hat voices | 29.75% | 26.13% |
| Median traced graph wall time | 3.084 ms | 2.638 ms |
| Median traced Space Echo job | 802 µs | 430 µs |
| Median summed traced kernel time | 9.727 ms | 9.288 ms |

All other engine voice counts matched throughout. The changed app had twelve
hat voices in 28 of 30 readings and ten/eleven in the remaining two. Excluding
those two gives the reported 12.2% improvement; the full interval gives 12.3%.
Its measured process CPU was 140.1%, but the second baseline run did not have
a matching cumulative process-CPU observer, so a separate process-CPU reduction
is not claimed for this DSP change.

A subsequent set of forty graph captures confirmed that the saving occurs in
the final dependency chain. The hat/FilterTable/Space Echo chain remained last
in every slice. Trace timing includes instrumentation overhead and covers graph
processing rather than the complete callback. The transport comparison uses
the app's existing averaged monitor, sampled every two seconds. This is one
before/after app comparison, supported by the repeated isolated kernel benchmark,
not a randomized multi-run project experiment or an audio-dropout certification.

### Remaining costs

Further hi-hat work is tracked in `eseq-t9nx`. Its twelve enabled voices remain
part of this workload. Adding a generated-C pointer alias annotation made no
measurable improvement. Explicitly moving three latch conditions outside their
element loops showed only a small preliminary gain; neither experiment changed
production code. A proper compiler optimization must preserve frame-aware
coefficient materialization and all contact/decay behavior.

### Autonomous FilterTable follow-up

The completed spring and graph-profiling pass was committed as `d82edb65`.
The next pass used `tools/audio-experiments/compare_filter_table.py`, without
opening the UI. It compiles the unchanged complete production causal effect,
loads the native ABI with the app's Accelerate FFT services, and checks the
output before timing. Flat/shaped procedural magnitude banks stand in for the
project's table; this is an isolated effect workload, not a replay of mow B2.

The first compiler experiment published reductions through local accumulators.
It produced identical audio but no measurable speedup: static 285.43 -> 285.23 µs,
automated 282.04 -> 283.71 µs. The uncommitted experiment was removed. Generated
C that appears inefficient is not sufficient evidence of a runtime bottleneck.

The successful change is in the adjacent DGen compiler's
`Sources/DGen/IRBuilder+ViewTransforms.swift`. A circular window's normalized
write head and valid element index bound the unwrapped index to
`[1 - windowSize, bufferSize - 1]`. Only a negative wrap is possible. Replacing
the per-tap integer remainder with one conditional addition saves work while
retaining every tap, the 8 ms kernel slew, and the existing hop cadence.

Nine alternating native repetitions, after two discarded rounds, at 48 kHz
and 512 frames measured:

| Full causal FilterTable kernel | Pinned compiler | Local candidate | Reduction |
| --- | ---: | ---: | ---: |
| Static controls | 280.48 µs | 249.45 µs | 11.1% |
| Frame/cutoff/resonance automation | 299.76 µs | 264.99 µs | 11.6% |

Each timed invocation has 256 warmup blocks (2.73 seconds of audio) and 2,048
measured blocks. Timing uses native thread CPU time; Python, compilation and
initial FFT setup are excluded. No build from this task overlapped the final
timing run. Other desktop work may affect CPU frequency, so alternating runs
and the retained individual measurements matter more than a single absolute time.

All eight old/new waveform comparisons (two banks × static/automated controls ×
regular/irregular partitions) were **sample-identical**. Generated-code fusion
checks were clean. Three targeted compiler tests passed: a new exact circular
buffer regression covering 15 block/window combinations and several wraps,
the stored-operand fused reduction regression, and buffered FFT/IFFT execution.
The compiler release build also passed. No full suite or Linux performance
claim is made.

Irregular short calls exposed an existing limitation: the shaped-bank output
differs between regular and irregular partitions in both old and new compilers.
A smaller counter/buffer probe reproduced identical failures on unchanged
compiler source. This extends the existing `dgen-j6r` investigation, with host
follow-up `eseq-mi4l`; it is not introduced by the optimization. Passing the
compiler comparison does not certify partition invariance.

The compiler change is committed upstream as
`db6065ec87aa4b95a9e99563a66380ef8f8d86ec` and published in
[DGenLisp v0.1.24](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.24).
Eseq's macOS arm64 pin now selects that release; the Linux pin remains v0.1.20.
The archive SHA-256 is
`ca0f0cfae48c597a07b4d0dc3b758a4e75edf13f6b3f38bc372963d1e8eb7b71`.
The fetched package matches the tested stripped, ad-hoc signed distribution
byte for byte, and its generated FilterTable C exactly matches the benchmarked
candidate. Both the staged package and the fetched default compiler passed four
focused host tests covering FIR tap placement, hop updates, causal unity and
cutoff response. The fetched run used no compiler, audit or runtime-header
environment overrides.

Newly loaded instruments/effects use the fetched compiler; already-loaded
instances need a project reload. No app rebuild is required to select it.
This follow-up adds **no new measured app transport reduction** beyond the
26.13% result above. With four workers, an 11% effect saving cannot be treated
as an 11% callback saving.

The full machine-readable result is
`tools/audio-experiments/filter-table-window-results.json`. Native C, manifests,
audio arrays and the rejected compiler experiment are retained under
`.local/benchmarks/mow-2026-09-13/filter-{window-final,reduction}/`.

Raw evidence is retained under `.local/benchmarks/mow-2026-09-13/`:
`confirmed-live-sample.txt` and its JSON attribution, `ui.stderr.log`,
`live-polyphony-observation.jsonl`, `live-polyphony-summary.json`,
`mono-critical-1.jsonl` and its analysis/Chrome trace, `spring-pair-probe.log`,
`spring-pair-verify.log`, `spring-pair-tests-library.log`, preserved
project/compiler artifacts, and the isolated Modal Kick candidate and checks.
`mono-spring-comparison.json` and both monitor logs retain the live comparison;
`mono-critical-paired.jsonl` contains the second trace. The ordinary release app
is also rebuilt with tracing compiled out.
The saved project was not rewritten by the profiling tools. Investigation
issue: `eseq-wlhz`; the remaining compiler work is tracked separately above.
