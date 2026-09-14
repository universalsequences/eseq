# garageddd B11 audio timing and heap audit — 2026-09-14

After the rack, phaser and scheduled-event fixes, three warmed B11 captures
recorded **zero Rust allocations, reallocations and frees** across 16,874 blocks
(179.989 seconds), covering the complete callback and all four DSP helpers.
This is an exact allocator count for the executed workload, not a proof about
unexecuted scenes or interactions. Loading and native graph growth are explicitly
outside the user's requested scope. One 11.669 ms callback still exceeded the
10.667 ms deadline, so this result does not establish that every glitch is fixed.

## Reproduction and scope

Frozen project: `.local/benchmarks/garageddd-b11-2026-09-14/project.json`.
Bank B scene 11 is global one-based scene 23. The project has 17 tracks at
156 BPM. The headless experiment uses the production CoreAudio callback,
scheduler, four DSP helpers, 512-frame blocks and 48 kHz sample rate. The block
deadline is 10.667 ms. Device output is silenced after DSP. No existing app
session is controlled and no project is saved.

B11 does not depend on project scratch being evaluated by the UI authoring VM,
so that known headless-harness limitation does not invalidate this workload.
The saved project is frozen; source and native libraries are recorded with the
individual builds. Allocation runs are not timing comparisons: sanitizer
reporting and other concurrent workspace activity perturb scheduling.

## Timing evidence and rack fix

The initial three one-minute captures contained 16,867 blocks / 180.012 seconds:
mean 3.096 ms, p99 6.994 ms, worst 26.011 ms. Four blocks exceeded the deadline.
The transport meter smooths load with 97% previous / 3% current weighting. It
cannot expose worst-case callback cost. The existing slow-block warning measures
only the graph-render portion, omitting scheduled-event dispatch before it.

Opt-in phase/event instrumentation localized one 28.383 ms callback to a
22.034 ms rack-parameter event on zero-based track 10. Removing that rack copy
then exposed another 21.974 ms rack note-trigger copy. All affected rack paths
now retain the parent snapshot Arc and borrow rack data. Macro resolution uses
bounded scalar overlays, parameter ordering uses bounded scratch, and rack
release/choke collection uses fixed-capacity storage derived from voice limits.

After the rack fixes, another three minutes contained 16,874 blocks: mean
3.086 ms and maximum 11.971 ms, with one deadline miss in DSP rendering. The
largest individual event was 60.666 microseconds. These results precede the
additional phaser fix and do not establish that all glitches are solved.

Nine focused rack/routing tests passed. They include a deep-copy allocation
positive control, maximum parameter counts, real graph dispatch, held/live macro
updates, note allocation, retrigger and release. The 80-second deterministic
offline render after the final rack changes matched the original binary exactly:

```
33debd98ea06a4eea2af9ced563e2d3952a29837ff32d7020adf004697bb9092
```

## Broader native audit

`audio-rtsan` integrates the pinned RealtimeSanitizer runtime through
`rtsan-standalone` 0.3.0. Scopes cover the outer application-owned CPAL callback,
shared rendering, and native worker lifetimes. Feature builds require
`RTSAN_ENABLE=1`; unsupported or silently disabled builds are rejected.

Calibration detected Rust allocation/free and native malloc, calloc, realloc,
posix_memalign and free from a library loaded after startup. The stack-only
control reported zero. However, direct macOS malloc_zone_malloc/free were
**not intercepted**. This gap is retained in the report, and a clean run cannot
certify absolute zero. Apple malloc_history also missed known positive controls;
xctrace could not load its Allocations template. Neither is used as zero evidence.

The first all-phase native run completed 20 seconds of warmup and 30 seconds of
measurement. It produced 126 heap reports among 164 distinct real-time reports.
The total 811,900 real-time violations includes synchronization and I/O; it is
**not an allocation count**. Confirmed call paths include:

- Phaser/flanger: two frequency-layout Vec allocations every 32 samples while
  phasing, followed by destruction, on the callback or DSP workers.
- Scheduled parameter dispatch: destruction of owned effect-parameter Vec
  payloads and allocation/free in stable sorting.
- Native graph editing during loading: node/port/edge allocation, cycle-check
  scratch, graph-capacity growth, buffers, and retirement.
- Native watched-state snapshots: allocation on first publication or size change.
- First-use callback Mutex storage and worker/callback thread-local storage.

The phaser frequency layouts now use ArrayVec at the existing 12-notch bound;
frequency math and modulation cadence are unchanged. Three focused tests passed,
including first-block heap checks across both circuits, all three device modes,
and minimum/maximum notch counts, plus existing acoustic-response tests.

The scheduled-payload findings are now addressed by the ownership protocol below.
The user subsequently clarified that loading, first-use setup and native graph
resizing are acceptable; `eseq-pwmc.2` was closed as outside that contract.
Preview replacement/end/stop remains a separate unexecuted interaction tracked
by `eseq-pwmc.3`. The invalid sampler modulation route is separately tracked by
`eseq-2pqj`. Neither is represented as covered by an untouched B11 playback run.

## Measured playback after the phaser fix

Three further runs each audited 60 seconds of wall time after 20 seconds of
warmup. Every run reported the same four heap stacks: two payload-free stacks
in scheduled event dispatch, plus malloc/free in stable sorting. No phaser heap
stack appeared. This is positive evidence that steady B11 playback still
violates the rule, independently of project loading.

| Run | Captured blocks | Rendered audio seconds | Heap reports | Total real-time errors |
| --- | ---: | ---: | ---: | ---: |
| 1 | 5588 | 59.605 | 4 | 192326 |
| 2 | 5601 | 59.744 | 4 | 173780 |
| 3 | 4748 | 50.645 | 4 | 162529 |

Totals include locks and I/O and must not be interpreted as heap operation
counts. Captured audio time is shorter than elapsed measurement time under
sanitizer/concurrent-machine load; these runs are not performance measurements.
The runner exited 1 as expected because it detected violations. All required
positive/negative controls passed; the macOS zone coverage gap remained visible.

The full 80-second offline render after **both the rack and phaser changes**
also matched the original SHA-256 above exactly. All 12 focused regressions
(nine rack/routing and three phaser tests) passed in their selected runs.

Both the audit feature build and `cargo build --release -p sequencer --bin
metal_seq` passed. Fixes in the rebuilt app take effect on its next launch; the
interactive app was not restarted by this investigation.

## Steady Rust audit after scheduled-event fixes

The queue now retains a scheduler-owned Arc for each admitted event. Callback
countdown/block queues hold immutable owners and borrow parameters during
execution. Only the producer reclaims completed payloads. Total in-flight events
are bounded by the existing queue admission limit, including events already
popped into countdown storage; exhaustion returns the rejected event to the
producer. There is no garbage-queue overflow fallback. Queue pop and clear use
ArrayQueue without taking the producer's reclamation mutex.

Effect parameters are stably sorted before admission, preserving same-target
precedence and dispatch-time reads of live send cells. Mutable callback routing
is separate from immutable payload ownership, preserving track-delete remapping
of delayed notes. Fixed-size voice release collections and descriptor-index
ordering also avoid Rust heap scratch.

The opt-in `audio-heap-audit` feature wraps all four Rust GlobalAlloc entry points.
It marks the complete CPAL callback and the native helper thread lifetimes, so
Rust DSP invoked by C is included. The counter is enabled after 20 seconds of
warmup and disabled at the end of measurement. Each process first verifies
allocation, zeroed allocation, reallocation and free in both marked thread roles,
plus exclusion of unmarked work. Calibration returns exactly two allocations,
one reallocation and two frees for each role. It also verifies that actual
playback enters all four native helpers. Normal builds omit these hooks.

The project file SHA-256 still matches the user's current garageddd save:
`091e93675a262046b7606c389899ecc03ba87ae5ace250fd35b1c5edc9270c41`.

| Capture order | Blocks | Audio seconds | Rust alloc/realloc/free, callback and helpers | Worst callback |
| --- | ---: | ---: | --- | ---: |
| 1 (r0) | 5625 | 60.000 | 0 / 0 / 0 | 7.381 ms |
| 2 (r2) | 5624 | 59.989 | 0 / 0 / 0 | 11.669 ms |
| 3 (r1) | 5625 | 60.000 | 0 / 0 / 0 | 7.635 ms |

All three runs passed allocator calibration and the runtime heap check. There
were 31,674 dispatched events, zero measured late/dropped events, and no
nonfinite samples. Overall callback mean was 3.112 ms and p99 was 6.524 ms.
The largest individual event took 53.333 microseconds. The single deadline miss
contained no scheduled event: DSP render took 10.783 ms and post-render work
0.854 ms. Further DSP/helper/OS scheduling diagnosis is tracked as `eseq-cczw`.
These are measurements on the shared machine, not an isolated scheduling proof.

Twelve focused queue/callback/routing tests passed, including concurrent reuse,
producer-only tensor destruction, in-flight saturation, stable effect order,
live sends, cancellation, range exclusion, and delayed track remapping. The
allocator assertions execute real graph dispatch, and the scheduler regression
uses the production lookahead routing harness.

The final offline render (20-second warmup, then 60 seconds of captured audio)
also counted zero Rust heap operations on the callback and all four helpers,
with zero late/dropped events. Its PCM SHA-256 matches the original baseline
exactly: `33debd98ea06a4eea2af9ced563e2d3952a29837ff32d7020adf004697bb9092`.
Evidence is in `offline-events-after` beside the other frozen captures. Offline
timings are not a realtime performance comparison; the normal app build ran
concurrently with that semantic check.

The final normal `cargo build --release -p sequencer --bin metal_seq` passed
in 2m45s. The existing interactive app was not restarted; the rebuilt fixes
take effect on its next launch. The scoped work is complete in `eseq-pwmc` and
`eseq-pwmc.1`; the preview and remaining timing follow-ups above stay open.

Raw logs, per-block JSON, the calibrated binary, build log, source diff and new
source files are archived under:
`.local/benchmarks/garageddd-b11-fix-2026-09-14/allocation-audit/rust-steady-after-events/`.

Repeat the Rust audit with:

```sh
cargo build --release -p sequencer --features audio-heap-audit --bin audio_experiment
python3 tools/audio-experiments/run.py \
  --project .local/projects/garageddd.json --pattern 23 \
  --names callback-wait-50-w4 --warmup-bars 13 --measure-bars 39 --repeat 3 \
  --out /tmp/garageddd-rust-audit
```

Those bar counts are 20-second warmup / 60-second measurement at this project's
saved 156 BPM. Each JSON result includes calibration, separate callback/helper
heap counts and `rust_heap_audit_passed`; a failed check exits unsuccessfully.

## Repeat the broader native audit

```
python3 tools/audio-experiments/audit.py \
  --project .local/projects/garageddd.json --pattern 23 \
  --scope measured --seconds 60 --repeat 3 --out /tmp/garageddd-audit
```

Use `--scope all` when lifecycle operations are the subject of investigation.
A warmed-playback result does not certify loading or shutdown; those operations
are outside the current request. The runner saves
binaries, runtime hash, calibration, raw logs and structured reports. Status 1
means detected real-time violations; 2 means no detected violation but incomplete
allocator coverage. Normal builds have no sanitizer hooks.

Raw evidence is under
`.local/benchmarks/garageddd-b11-fix-2026-09-14/`, including
`phases-before`, `events-before`, `updates-after`, `racks-after`,
`offline-before`, `offline-after` and `allocation-audit`.
