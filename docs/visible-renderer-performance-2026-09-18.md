# Visible renderer performance

This pass follows checkpoint `4a80e23d`, which contains the earlier retained
paint and scratch-only work. The follow-up is tracked as `eseq-6you`.

The user’s live trace exposed costs that paint-cache hit rates did not explain:
full layout scans for an obsolete animation and modulation ports, primitive
eligibility checks before compiled-cache lookup, small Metal submissions,
dynamic geometry preparation, and two new GPU buffers for each spectrum update.

## Changes

- Removed the agent instrument background animation and its layout/primitive
  scans, backend flags, and special event-loop frame cache in Metal and wgpu.
  Draft instruments retain a static placeholder through ordinary custom UI.
- Retained scenes propagate changes to ancestors and reuse the prepared run
  spans of unchanged branches. Each scene independently observes widget-state
  changes; resource checks visit registered resource readers. Stable interaction
  IDs, focus, hover, viewport, theme, shader, layout, and overlay changes remain
  invalidation inputs. Relative child spans remain valid when a sibling changes
  the number of emitted runs. Offscreen dirty paint remains pending.
- Metal retains translated CPU primitives and their supported/unsupported
  classification until geometry or presentation metrics change.
- Adjacent supported runs compile in bounded batches of up to 32 runs, within
  the existing clip and z-order barriers. Drawing phases preserve their previous
  ordering. Source revisions, identities, translations, metrics, and atlas/shader
  generations validate cache entries. Sparse changes rebuild affected batches.
- Empty phases skip encoding; adjacent compiled draws avoid redundant pipeline,
  texture, and translation bindings. Dynamic work resets this local state cache.
- Animated widget shaders use the current frame-time uniform with retained
  geometry. Their animation does not itself require vertex uploads.
- Spectrogram snapshots use immutable buffer leases from the existing bounded
  Metal pool. Both cached resources and submitted frames own leases, so replacing
  an analyzer revision cannot overwrite bytes still read by the GPU.
- Both backends index modulation port locations by an owned layout identity.
  Rendering still reads current port state and reactive signal levels. Layout
  replacement rebuilds the index; absent ports do not cause a per-frame walk.
- Per-segment renderer timing is enabled for diagnostics or explicit captures.
  Ordinary rendering retains the clocks needed for animation and scheduling.

## Verification

The focused renderer validation passes 38 tests, covering retained-scene
invalidation, sparse edits, changed run counts, offscreen reentry, nested clips,
scrolling, focus/hover, resource publication, independent widget state, buffer
ownership, and comparisons against fresh/dynamic Metal rendering. Additional
checks prove animated shader pixels change without geometry uploads and indexed
ports observe changing reactive levels without layout replacement.

The new clean-subtree reentry regression first reproduced a stale-span bug in
this pass, then passed after tracking whether a parent emitted descendant spans
in the previous frame. Culling invalidates those spans without discarding paint;
reentry rebuilds presentation metadata before branch reuse resumes.

Four release host checks pass: placeholder parsing, custom UI registration,
finite/nonzero placeholder geometry, and modulation-route hit testing.
Production `metal_seq capture` images were inspected for the static placeholder
and the saved PM Piano/EQ8 panel. The full replay capture additionally exercises
sequencer, mixer, scrolling, scratch-only display, reopening, compressor history,
and a changing EQ8 spectrum.

The wgpu feature compile check passes. The original shared-event GPU-fence
regression remains subject to the previously documented device limitation
(`eseq-wej3`); no test was weakened to hide it.

## Measurement scope

Artifacts and preserved before binaries are under
`.local/benchmarks/renderer-pass-2026-09-18/`. A 20-second live Time Profiler
baseline was captured from process 84761 without changing playback or layout.
The saved-project replay uses the frozen `garageddd` project plus nine empty
samplers, producing 28 tracks with all groups open. Its optional
`ESEQ_UI_REPLAY_SPECTRA=1` publication path adds changing data for mounted
spectrograms, alongside the existing compressor histories.

Replay timings measure production synchronization, frame construction, and
Metal preparation/submission. They exclude OS event dispatch, actual analyzer
processing, scheduler execution, audio deadlines, and display scanout. They must
not be presented as Activity Monitor percentages or end-to-end input latency.

## Results and real-app outcome

Two alternating runs of each preserved binary, with the same frozen project,
gave these pooled replay results:

| Replay phase | Before median | After median | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| Panels, total | 5.590 ms | 3.015 ms | 6.858 ms | 4.887 ms |
| Live panels, total | 5.828 ms | 3.102 ms | 7.607 ms | 4.338 ms |
| Scrolling, total | 5.984 ms | 3.823 ms | 7.583 ms | 5.356 ms |
| Scratch, total | 0.0346 ms | 0.0343 ms | — | — |

Live-panel render time fell from 4.002 to 1.615 ms median; retained-scene
preparation fell from 1.022 to 0.126 ms. The after replay still issued about
1,390 draw calls per live frame. These are useful component improvements, not
evidence of a proportionate whole-app improvement.

The live Time Profiler comparison was much less convincing. Before: 8.606 s
sampled main-thread time over 20.864 s. After: 7.580 s over 20.634 s, approximately
11% less after normalizing by trace duration. Renderer inclusive weight fell
from 3.116 to 1.543 s, while native polling increased from 2.199 to 3.395 s.
The captures were separate live runs and do not establish why polling increased.
They also have differing dynamic-widget costs, so individual stacks are not a
controlled factor-by-factor speedup measurement.

The user reports roughly unchanged feel and Activity Monitor readings. That
means the app-level performance goal remains open. A follow-up observation of
OS thread counters around stack sampling did not isolate sampling overhead:
main-thread CPU remained elevated afterward. We cannot attribute the mismatch
to Instruments alone.

The next acceptance metric is the [live main-thread CPU benchmark](ui-main-thread-benchmark.md),
tracked as `eseq-pzfv`, with native presentation and input-dispatch guardrails.
It includes the actual event loop and UI-side synchronization in the score.
Audio-thread optimization is explicitly outside this work's scope.

The new live command has now been exercised against foreground `garageddd`
playback: CPU-only readings were 238.4 CPU ms/s with panels and 28.4 with scratch.
Two panel runs with native presentation tracking measured 251.9–259.2 CPU ms/s
at 59.84–59.99 displayed FPS. These establish an initial current-build baseline;
there is no equivalent before-build measurement yet. The benchmark document
records accepted/excluded samples and the remaining calibration limits.

## Reactive subscription lookup follow-up

`eseq-qgpe` replaces the flat widget binding table with subscriptions grouped by
namespace and field. A demand check now uses borrowed-key hash lookups instead
of scanning every binding. Scalar and indexed publication also avoid building
temporary owned keys. Scalar readers are stored inline; only indexed bindings
need an index table. There is one source of truth rather than a separate reader
count cache to keep synchronized.

All replacement paths prune empty fields and namespaces, including tile deltas,
subtree replacement and snapshot restoration. Seven focused release registry
tests pass, covering scalar/indexed notification, deduplication, namespace
isolation, removal, full/subtree replacement, snapshot restore and cloning. Two
host regressions also pass: visualization publication requires live consumers,
and sequencer visibility reads the registry without invoking Lisp.

These results validate behavior, not a visible-app speedup. The preserved
`before-reader-index-metal_seq` binary contains the live benchmark endpoint for
a matched comparison. The earlier visible baseline cannot be compared directly
with the user's subsequently resized window and changed selected effect.

After restarting and restoring the original 2500×1700 window, 19 tracks and
selected track 0, two clean foreground playback samples measured **255.6 and
248.6 main-thread CPU ms/s** (mean 252.1). Both had zero input events, unchanged
workload snapshots, and exactly 2,400 host passes for 1,200 submitted frames in
20 seconds. A separate presentation run measured 60.00 displayed FPS, with
16.67/25.00/25.00 ms presentation-interval p50/p95/p99 and no missing or skipped
drawables. These readings do **not** establish an overall improvement from the
subscription change. Artifacts and the binary hash are recorded in
`live-ui-reader-index-comparison.json` beside the individual reports.

## Native-loop investigation

An isolated program linked against the app's winit 0.29.15 dependency reproduced
roughly two native-pump returns per 60 Hz deadline without a window, renderer,
project or audio. The current timeout policy generated thousands of repeating
timer callbacks while AppKit unwound the pump, using 114–130 CPU ms/s in this
minimal program. The program's counters and CSV output are diagnostic overhead;
these are not measurements of the running app.

Two attempted caller-owned timer policies reduced the isolated CPU score but
failed wake-up checks: a user event sent after 10 ms sometimes did not return to
the host until the 200 ms poll deadline. Both candidates were removed from
production sources before rebuilding the app. Their lower CPU scores are not
acceptable improvements, and no timer-policy change has shipped.

A separate native-owned `run_on_demand` loop measured 8.65 CPU ms/s for 300
60 Hz deadlines and received all 20 cross-thread notifications, with dispatch
delays of 0.02–0.09 ms from sending the notification. This isolates a promising
architectural direction, not an in-app speedup or input-to-display measurement.
The application still needs an explicit host-tick/native-event interface that
preserves queued input ordering, thread/menu wakes, deadline scheduling, file
drops and synchronous live resize. This remains tracked in `eseq-x3ix`.

Reproduction sources and logs are under the artifact directory:
`native-pump-probe.rs`, `native-pump-wake-check.rs`,
`native-loop-ownership-probe.rs`, and their CSV/log outputs. Rejected timer
helpers are retained only there so the wake regression can be reproduced.

## Native-owned application loop

The macOS application now enters `run_on_demand` once. A small host schedule
requests application work for translated input, cross-thread notifications,
remaining queued input, or the host's absolute deadline. Internal native wakes
before that deadline do not repeat reactive synchronization. The sequencer still
owns its 60 Hz frame pacing and 50 ms idle servicing; a late frame advances to
the next deadline without a catch-up burst. Linux retains the existing polling
entry point.

Native input translation is shared with the old polling backend. File drops
remain grouped per native batch, the final coalesced drag precedes release, and
queued input resumes immediately after a host-command ordering barrier. Resize
still draws synchronously inside the native callback. MIDI, menu and benchmark
notifications use the existing event-loop proxy. No audio-thread work changed.

The production driver has a main-thread integration probe:

```sh
cargo run --release -p eseqlisp --features capture-harness \
  --example native_host_loop_probe
```

It uses a hidden window with activation prohibited, no project or audio, and no
synthetic OS input. It checks 20 thread notifications interrupting 200 ms idle
waits, queued keys delivered across separate host ticks, a real window-size
request and synchronous resize callback, 60 paced deadlines, queued quit, and
host error propagation after re-entering the native loop. The initial run passed
with notification delays of 0.012–0.125 ms. This validates the driver contract;
it does not measure input-to-display latency or prove heavy-project performance.

Four focused backend release tests also pass, covering spurious wakes, deadlines,
proxy/input requests, queue priority and drag/release ordering. Thirteen targeted
`metal_seq` release tests pass for frame deadlines, host input ordering, benchmark
CPU/presentation boundaries and IPC. The wgpu feature check and normal release
build also pass. These automated checks establish behavior; the live garageddd
comparison below measures performance.

After the user restarted the release build, PID 30548 ran the native-owned loop
with the same recorded workload as the prior baseline: 19 tracks, selected track
0, all seven UI buffers, 2500×1700 physical pixels and 156×48 cells, playing,
focused, visible and unoccluded. All three new 20-second samples had zero input,
unchanged workload snapshots, no diagnostic flags and no lost presentation
feedback. No builds or stack profilers ran during measurement.

| Measurement | Previous polling loop | Native-owned loop |
| --- | ---: | ---: |
| CPU-only main-thread CPU ms/s, run 1 | 255.6 | 198.7 |
| CPU-only main-thread CPU ms/s, run 2 | 248.6 | 204.0 |
| CPU-only mean | 252.1 | 201.4 |
| Reactive syncs per second | 120 | 60 |
| Displayed FPS, separate presentation run | 60.00 | 59.99 |
| Presentation interval p50/p95/p99, ms | 16.67 / 25.00 / 25.00 | 16.67 / 25.00 / 25.00 |

The measured CPU reduction is **20.1%**, or **50.7 CPU ms/s** (about 5.1
percentage points of one core). The duplicate host passes are gone: each new
sample has one reactive sync per submitted frame. This is a real-app CPU result,
but presentation-interval tails have not improved. These sequential samples are
not a randomized crossover, and matching recorded fields does not prove every
internal project state is identical. No input occurred, so interaction latency
and subjective responsiveness remain unvalidated. Audio remains out of scope.

The new release SHA-256 is
`c7b30fcee846dee13a9c853914e804d2dbbab9994111bc3a30900889246a25f6`.
`live-ui-native-loop-comparison.json` records both binary hashes, accepted
reports, matching workload and exact values alongside the individual reports.

## Editor housekeeping follow-up

The subsequent [housekeeping comparison](editor-housekeeping-performance-2026-09-18.md)
measured 239.7 to 205.5 main-thread CPU ms/s (14.3% lower), at approximately
60 FPS with the same panel throughout that comparison. Status-row construction
showed the clearest saving; separate metadata and shader-notification benefits
were not established. This is a new workload comparison, not a continuation of
the earlier 201.4 CPU ms/s sample with a different selected panel.
