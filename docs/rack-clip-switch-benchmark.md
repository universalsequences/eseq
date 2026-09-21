# Rack clip switch benchmark

`tests::saved_project_rack_clip_switch_benchmark` measures clicking a clip square
in a collapsed drum rack header, using a real saved project. It is an ignored,
macOS-only release test in `metal_seq`; it does not change app behavior.

Run from the repository root (absolute paths are required because the shared
test fixture changes its working directory):

```sh
ESEQ_RACK_CLIP_PROJECT="$PWD/.local/projects/jungle2-0.json" \
ESEQ_RACK_CLIP_OUT="$PWD/.local/benchmarks/rack-clip-switch/stopped.json" \
ESEQ_RACK_CLIP_PLAYING=0 \
cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=tests::saved_project_rack_clip_switch_benchmark)' \
  --run-ignored only --no-capture

ESEQ_RACK_CLIP_PROJECT="$PWD/.local/benchmarks/rack-clip-switch/stopped.project.json" \
ESEQ_RACK_CLIP_OUT="$PWD/.local/benchmarks/rack-clip-switch/playing.json" \
ESEQ_RACK_CLIP_PLAYING=1 \
cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=tests::saved_project_rack_clip_switch_benchmark)' \
  --run-ignored only --no-capture
```

The second run reuses the first run's snapshot so concurrent saves cannot change
the comparison. Check both reports' `project_sha256` values match. The saved
project and sample library must be available locally; no samples are replaced.
Run alone without another build or benchmark for meaningful comparisons.

The fixture copies the project bytes before loading, restores rack graph scripts
and tabs, and collapses the rack through the production
native. It uses the normal multi-panel startup layout, application font and Metal
renderer at 2500 × 1700 pixels, Retina scale 2 (override with
`ESEQ_RACK_CLIP_WIDTH`, `ESEQ_RACK_CLIP_HEIGHT`, `ESEQ_RACK_CLIP_SCALE`).
It retains the project's saved track cursor. Clip launches have quantization **off**;
musical boundary waits are outside this benchmark's scope.

The timer starts immediately before pointer-down dispatch and stops after the
updated frame's Metal command buffer has completed. Each sample includes:

- Precise tiled pointer down/up dispatch and Lisp callbacks.
- The actual `launch-rack-clip` host handler, including history and scene relaunch.
- The production reactive tick.
- Full tiled frame construction and Metal rendering, including GPU completion.

Setup, project load, coordinate discovery, correctness checks and output files
are outside the interval. The first click is reported separately. Two further
warmups are retained in the JSON, then one bank's worth of different clip
launches produces min/median/p95/max for the total and each phase. Set
`ESEQ_RACK_CLIP_SAMPLES` to override the sample count (minimum two), or
`ESEQ_RACK_CLIP_GROUP` to select a rack ID when a project has multiple racks.

Every click must emit exactly one launch for the intended rack/clip. The fixture
checks the model's active clip, installed live lanes, unchanged global scene, every clip's rendered
active prop and the number picker's value. It also requires finite visible hit
geometry and absence of the collapsed members' step grids. Assertions fail
instead of timing a missed click, a queued quantized launch or a no-op.

Outputs alongside the JSON are the exact input `*.project.json` and PNGs before
and after the first completed click. The report includes raw samples, UI rerun/relayout
counters and Metal timing/caching counters. The test has no hardcoded,
machine-independent ceiling; the measured baseline and agreed local budget
are recorded below.

`ESEQ_RACK_CLIP_PLAYING=1` exercises the playing **UI state**. The existing
headless audio graph pump services DSP commands; this test has no scheduler,
hardware audio device, OS input queue or display scanout. It measures application
click-to-frame latency, not click-to-audible-sound latency or a hardware input
latency. Treat comparison with a busy interactive app accordingly.

## Initial baseline, 2026-09-20

Apple M1 Max, 32 GiB, macOS 26.5.1; release build of the working tree based on
`267556122bcd8595a30774f0acb66ef7f6e88b9f`. The final verification ran the exact
test above separately in each transport state. Both passed; the captured frame
was inspected. Project loading reported zero fallback samples.

The frozen `jungle2-0` input has 12 tracks, five scenes, saved track cursor 2,
current scene 2 (both zero-based), and rack 1, “Jungle-ology Kot”, with eight
members and 22 clips. Input SHA256:
`2815b065ba56c81265aa95b9a762f9e85516f230edf349c2e36b5dfa84f0f2bd`.
These runs use 2500 × 1700 pixels at scale 2, with the rack collapsed.

| Transport UI state | First click | Median, 22 switches | p95 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| Stopped | 301.2 ms | 335.6 ms | 437.9 ms | 491.8 ms |
| Playing (headless) | 275.5 ms | 304.1 ms | 476.3 ms | 523.1 ms |

About 93% of measured time is in the production host-command handler. Its
median is 312.5 ms stopped / 280.8 ms playing; the reactive tick is
14.5 / 14.8 ms and completed Metal rendering is 7.1 / 7.8 ms. Phase medians
need not sum to the median total. Runs varied with desktop activity, so these
numbers do not establish that one transport state is faster.

This **does not reproduce the reported 600–750 ms delay**. The app's existing
read-only diagnostic endpoint reported a 17-track running session during this
investigation, whereas the saved input has 12 tracks. Matching that session,
and measuring the full live scheduler/input/presentation path if needed,
remains necessary before claiming the user's complete delay is reproduced.
This baseline was collected before the production performance changes below.

Evidence is under `.local/benchmarks/rack-clip-switch-2026-09-20/`:
`baseline-stopped.json`, `baseline-playing.json`, corresponding logs/PNGs and
project snapshots, `environment.json`, `final-source-hashes.json`, and
`live-window.json`. Earlier `stopped`/`playing` reports used scale 1 and cursor
0; use the `baseline-*` reports for the final fixture. The earlier
`retina-*` reports are exploratory runs, not a performance comparison.

## Optimization, 2026-09-20

The agreed target is a 10× reduction in p95 on the same frozen input: at most
43.7935 ms stopped and 47.6322 ms in the playing UI state. The timer boundaries,
warmups, 22-switch sequence, saved selection, collapsed rack, and Retina viewport
remain unchanged. Use the original `baseline-stopped.project.json` for both
transport states, not a newly saved live project.

The final independent release runs passed the unchanged benchmark:

| Transport UI state | Baseline p95 | Final p95 | Speedup | Final median |
| --- | ---: | ---: | ---: | ---: |
| Stopped | 437.935 ms | 39.015 ms | 11.22× | 35.106 ms |
| Playing (headless) | 476.322 ms | 40.145 ms | 11.87× | 35.897 ms |

First clicks were 50.750 ms stopped and 54.868 ms playing; these remain separate
from the warm distribution, as in the baseline. Longer verification runs with
`ESEQ_RACK_CLIP_SAMPLES=200` also met the budgets: p95 39.651 ms stopped and
39.233 ms playing, with maxima 40.324 and 40.178 ms. No builds or other benchmark
runs overlapped these final measurements. A separate app build overlapped an
earlier stopped run; that report is retained as `contended-stopped`, not used
for the final comparison.

A comparison of the reports checked the fixture hash, viewport, selection, scene,
clip order, transport state and sample counts against the baseline, and asserted
both the 22-switch and 200-switch p95 budgets; its results are recorded in
`final-comparison.json`. The timed probe source itself is
byte-for-byte unchanged from the baseline (SHA256
`83158b0f7340808064bdfe998bb4ff5d0765da148daff55c7c38917678f49754`).
The shared test wrapper also matches its baseline source hash. Concurrent edits
to the editor/layout backend and mixer hit tests appeared after measurement;
they were left untouched and are listed in `measurement-provenance.json`.
These results identify the captured test binary, not those later renderer edits.

The production changes remove redundant work along the measured path:

- Clip assignment history stores a stable scene ID and before/after clip IDs.
  Save-back still preserves the outgoing clip's edits, and undo/redo restores
  playback when that scene is current.
- Descriptor synchronization preserves normalized grids and shared sound patches.
  A cache identifies immutable patch allocations with weak references and checks
  the complete binding layout; mutation or schema changes invalidate that result.
- A launch publishes one final scheduler snapshot after its graph bindings,
  defaults and manual override pins agree. Pinning an override reads IDs directly.
- Consuming a scene snapshot moves its owned track grids into stored patterns.
  Rack default/solo dispatch copies only the values it sends to the graph.
- Clip rosters are structural UI data. Highlights, the number picker, mixer play
  markers and scroll-follow use retained numeric bindings. Clip launches no longer
  trigger a topology refresh.
- Step bindings share reconciled p-lock render data and masks, avoid duplicate
  parameter-list publication, and encode their exact signatures without per-value
  formatting calls.
- The rack preset catalog retains encoded file bytes and validated names, avoiding
  repeated deserialization while still observing external content changes,
  replacements, additions and deletions.

Optimization reports and profiles are under
`.local/benchmarks/rack-clip-opt-2026-09-20/`. The benchmark measures completed
application frames; the headless scheduler/audio limitations above still apply.

Validation includes 10 focused model/scheduler tests (clip history, project
round-trip, launch timing, routing, binding identities, snapshot ownership and
rack gain/mute/solo) and three UI regressions (retained rack clip controls with
finite geometry inside the panel, existing step bindings and p-lock masks).
Descriptor normalization and preset catalog invalidation tests also passed.
Logs are `final-regressions.log`, `contended-stopped.log`,
`identity-coalesced-stopped.log` and `small-payloads-stopped.log`.

The production capture used `project-rack-clips.lisp` with a copy of the frozen
project whose rack collapse flag was set to true. The header and mixer were
inspected in `final-capture-clips.png`; the measured replay's captured frame
also verifies the clip changes visually. The 808 Kick host instrument probe
passed for 4,096 frames: peak 0.6294, RMS 0.3620, no non-finite samples/state.
Invoke direct instrument files with an absolute path:

```sh
cargo run --release -p sequencer --bin instrument_probe -- \
  "$PWD/content/instruments/Drums/808 Kick/dsp.lisp" \
  --frames 4096 --min-peak 0.01 --min-rms 0.001 --json
```
