# Live UI benchmark

`metal_seq benchmark-ui` measures the **existing interactive app**, with its
current project, playback, layout, and native event loop. It does not launch an
audio device, reload a project, sample stacks, or enable detailed renderer logs.
The app must have been restarted once with a build containing this endpoint.

```sh
target/release/metal_seq benchmark-ui --pid <app-pid> \
  --seconds 20 --warmup 3 --cpu-only --label garageddd-panels \
  --out /tmp/garageddd-panels.json
```

Use `--cpu-only` for the least-instrumented CPU score. Repeat without it to
collect native presentation cadence and input-dispatch guardrails. Keep the
mode the same when comparing CPU scores across builds.

Find the interactive process with `pgrep -fl metal_seq`. Run the command from
another terminal while keeping the app visible. The default three-second warmup
allows time to focus the app. Subsequent measurements require no restart. Use a
release build; the JSON records whether the measured app is a debug build.

## The number to reduce

**Main-thread CPU milliseconds per wall second** is the headline:

```text
1000 × (ending thread CPU seconds − starting thread CPU seconds)
     / elapsed wall seconds
```

For example, 300 CPU ms/s is 30% of one core. Lower is better **for the same
workload and presentation cadence**. It includes native event processing,
reactive synchronization, frame construction, rendering submission, allocation,
destruction, and any other work executed on the main thread during the window.
Sleep, blocked waits, GPU execution, and CPU work on other threads are excluded.
This is not the whole-process percentage shown by Activity Monitor. Audio and
its real-time threads are outside this measurement and are not instrumented.

The main thread reads `CLOCK_THREAD_CPUTIME_ID` at the start and end of a timed
window in its normal outer event loop. No CPU timer is read per widget or per
draw. If an iteration overruns the requested duration, both the actual CPU and
wall duration include that overrun. Report aggregation and file writing happen
after the CPU window; the CLI writes the output file.

## Guardrails and limitations

The report also includes:

- Actual Metal drawable presentations per second and presentation-interval
  p50/p95/p99, using the native drawable's presentation time. GPU submission and
  presentation-handler delivery are not treated as the display time.
- Software event dispatch to the next displayed frame started after that event,
  for events handled while a redraw is pending. Latency p50/p95/p99 is **null**
  when there are no input samples. This is a scheduling diagnostic, not a claim
  that a particular pixel changed. It excludes OS input delivery before dispatch
  and the physical display's response; steady playback alone does not validate
  interaction feel.
- Loop iterations, event polls (including empty/zero-timeout polls), delivered
  events, reactive syncs, attempted frames, and submitted frames. These help
  explain CPU spent outside the renderer without enabling per-stage clocks.
  Schema 2 records `event_loop_mode`: on macOS, `native-owned` input polls are
  nonblocking reads of the translated event queue, not calls into AppKit. Native
  dispatch runs between host ticks and remains included in the CPU score. Poll
  counts must not be interpreted as native wake counts across this transition;
  host tick and reactive-sync counts still measure the actual application passes.
- Start/end snapshots of track count, selected track, playback, visible buffers,
  viewport size, window size, focus, visibility, and reported occlusion. A changed
  snapshot is flagged. These are bookends, not a complete history of project edits
  or a guarantee that the workload stayed constant between them.
- Missing/skipped presentation feedback, bounded-queue/sample overflows, and
  known enabled diagnostic flags. Inspect these before accepting a comparison.

Metal reports a zero presentation time for a dropped/unpresented drawable;
these frames cannot satisfy an input-latency sample. See Apple's
[presentation-time documentation](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime).
The benchmark waits asynchronously for up to one second after the CPU window for
outstanding feedback. Late presentation of a frame begun within the measurement
still counts; frames begun during warmup do not. Linux currently reports thread
CPU and loop counters, with native presentation/latency fields unavailable.

Instrumentation is opt-in per measurement. Disabled renderers allocate no
presentation callbacks, and the UI loop only checks whether a run is active.
During a normal measurement there are small per-frame callback/channel and
timestamp costs. To quantify them, repeat with `--cpu-only`, which retains the
CPU score and loop counters but omits drawable feedback and input timestamps.
Do not compare a CPU-only run's absent display guardrails to a full run as though
they were equivalent responsiveness measurements.

## Comparing changes

Use the same saved project, open groups, selected FX, viewport, playback state,
display, and foreground/occlusion state. Avoid builds and other profiling tools
during the window. Repeat each condition several times; compare medians and the
run-to-run spread, alongside presentation cadence and interval tails. During an
interaction run, repeat the same scroll/drag actions and inspect input latency.
A lower CPU score achieved by fewer presentations is not sufficient evidence of
an improvement.

For the scratch comparison, keep playback/project state unchanged, show only
scratch, then measure again with another label. The score difference estimates
the main-thread cost of displaying the panels in that workload. Scratch still
has a native window/event loop and UI-side host work, so it is a measured baseline,
not a literal zero-UI process.

The old saved-project replay remains useful for isolating renderer regressions.
It cannot establish an improvement in real-app CPU or interaction feel: it omits
the native event loop and real input delivery and runs a different workload.

## Endpoint

The interactive app listens on a Unix socket under a private, same-user temporary
directory, with one bounded measurement request in flight. Requests contain only
duration, warmup, label, and CPU-only mode. They cannot evaluate code, change the
project, or write arbitrary files from the app. The CLI chooses and writes JSON
output. The listener sleeps when unused; overlapping/invalid requests fail
without starting another measurement. Closing the app terminates a pending run.

## First live baseline, 2026-09-18

The command was exercised against the real release app, PID 13704, with
`garageddd` playing, 19 tracks, selected track 0, and the window focused and
unoccluded. Clean samples had zero delivered input/window events, unchanged
start/end workload snapshots, and no other UI diagnostic flags enabled.
These are observations of the current build, not a before/after speedup claim.

| Condition | Main-thread CPU ms/s | One-core equivalent | Displayed frames/s |
| --- | ---: | ---: | ---: |
| Panels, CPU-only | 238.4 | 23.84% | Not observed in this mode |
| Scratch only, CPU-only | 28.4 | 2.84% | Zero submitted frames |
| Panels, presentation tracking, run 1 | 251.9 | 25.19% | 59.99 |
| Panels, presentation tracking, run 2 | 259.2 | 25.92% | 59.84 |
| Scratch only, presentation tracking | 39.0 | 3.90% | 0 |

The CPU-only panel-minus-scratch difference was **210.0 CPU ms/s**, or about
21 percentage points of one core. With presentation tracking it was about
216.5 CPU ms/s. The CPU-only conditions each have one clean 20-second sample;
this is an initial baseline, not a confidence interval. Scratch was two physical
pixels taller (2500×1702 versus 2500×1700); both had a 156×48-cell viewport.

Neither clean panel presentation run lost a drawable or feedback record.
Presentation intervals had p50 16.67 ms and p95/p99 approximately 25 ms. Near
60 FPS therefore does not imply perfectly even frame spacing. Input latency
is unavailable for these idle samples; interactive feel is not validated here.

The visible layout ran roughly **120 empty event polls and reactive syncs per
second for 60 frames**. Scratch still ran about **80 polls/syncs per second**
while submitting no frames. The native-loop investigation is tracked as
`eseq-x3ix`; these counters identify repeated work, not its root cause.

Unfocused, occluded, resized, and input-containing samples were kept as artifacts
but excluded from the stationary foreground comparison. A separate unfocused,
visible sample measured 330 CPU ms/s and 54.45 displayed FPS; it must not be
pooled with the foreground baseline or attributed solely to profiler overhead.

Observer overhead is **not yet isolated** from run-to-run variation. Even the
two zero-frame scratch runs differed by 10.6 CPU ms/s, despite issuing no drawable
callbacks in either run. Do not infer the cost of presentation tracking from the
small set of sequential samples alone. Use repeated CPU-only runs as the primary
optimization score and collect display guardrails separately.

Artifacts: `.local/benchmarks/renderer-pass-2026-09-18/live-ui-*.json`.
`live-ui-comparison.json` records the accepted files, exclusions, values, and
release binary SHA-256 (`124393724fb0645aed2b690bc9be5e2c970883f5f37346f9195b90d60cf51203`).
No audio or real-time thread instrumentation was added.
