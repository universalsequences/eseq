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

The causal FilterTable's reduction code is the next compiler target
(`eseq-wlhz.3`). Its 256-tap reduction still publishes through cross-block scratch
inside the sum loop. Local accumulation with one final publication needs a
compiler-level implementation and complete waveform/modulation validation.
Generated-C edits and source tricks to coerce code generation are not shipped.

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
