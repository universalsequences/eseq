# Saved-project scene launch baseline

`app::launch_benchmark::saved_project_immediate_launch_latency` is an ignored
release library test. It measures the default **quantization-off** launch path
on a real saved project, including scene restoration, rack binding, defaults,
history for rack clip assignment, and scheduler snapshot publication. A separate
timer measures the production scheduler driver's first refill after each launch.
It does not change production playback behavior.

## Run

Use a frozen project and an absolute, new output path. Samples and instruments
must exist locally; missing samples fail the load instead of being substituted.
Run without overlapping builds, tests, or other heavy workloads.

```sh
ESEQ_LAUNCH_PROJECT="$PWD/.local/benchmarks/scene-launch-gotham-2026-10-12/fixture.json" \
ESEQ_LAUNCH_OUT="$PWD/.local/benchmarks/scene-launch-gotham-2026-10-12/immediate.json" \
cargo nextest run --release -p sequencer --lib \
  -E 'test(=app::launch_benchmark::saved_project_immediate_launch_latency)' \
  --run-ignored only --no-capture
```

The test copies the input to `immediate.project.json` before loading and records
its SHA256. The output includes raw timings, the exact target order, project
dimensions, tempo, sample rate, block size, and p50/p95/p99/max per rack or scene
category and visit round. Incomplete runs keep `complete: false` and cannot be
accepted by the comparison tool.

The sequence visits all scene targets and each rack's clip bank in fixed shuffled
order. Consecutive no-ops are skipped. Two rounds are the default: the first
includes first visits, the second revisits them. `ESEQ_LAUNCH_ROUNDS` can increase
this (minimum two). The currently loaded scene is launched once during setup;
this is not a cold process-start benchmark. No target-wide preflight warms all
destinations before measurement.

The fixture restores the project scratch and instance kinds, publishes their
sequencers, and checks scheduler errors and queue rejections. It rejects projects
with legacy rack-authored script lists instead of silently omitting their UI
publication step. After every launch, it checks the selected scene, live rack
clip, composed lane silence, note bits and step parameters against the pattern
pool. Validation, file writes and headless graph servicing are outside timers.

## What the numbers mean

- `assign_ms`: outgoing save-back and recorded rack pointer assignment (near
  zero for full-scene launches).
- `request_ms`: immediate production `App::apply_manual_pattern_launch`.
- `control_ms`: assignment plus launch; the synchronous work before the new
  snapshot is available. It excludes input dispatch and UI rendering.
- `scheduler_passes[].wall_ms`: the complete production scheduler advance,
  using a 512-frame block and four-block lookahead at 48 kHz. Event inspection
  and release are outside this timer.

The scheduler uses a deterministic sample cursor. There is no DSP rendering,
hardware device, wall-clock pacing, or concurrent UI/scheduler contention in this
test. The cursor advances one block per pass; the immediate workload stresses
rapid arbitrary launches rather than dwelling in each scene. Events are drained
without sounding them. These measurements expose work
added to launch/preparation and refill; they **do not establish audio underrun
rates, click-to-sound latency, or a guarantee that playback deadlines are met**.
Keep the existing click-to-completed-frame benchmark in
`rack-clip-switch-benchmark.md` for UI coverage. Neither substitutes for live
audio deadline measurements when changing the real-time handoff.

## Compare

```sh
python3 scripts/compare_scene_launch_benchmarks.py \
  .local/benchmarks/scene-launch-gotham-2026-10-12/immediate.json \
  /absolute/path/to/after.json
```

The comparison rejects different fixture hashes, launch orders, configurations,
sample counts, scheduling frontiers/event counts, and failed runs. It checks control, request, and scheduler p50,
p95, p99 and maximum. Default noise tolerance is the larger of 10% or 0.5 ms;
override with `--percent` and `--noise-ms`. This is a regression alarm, not a
permitted degradation target. Repeat a flagged run on an idle machine and inspect
the raw samples. First-round p99 with fewer than 100 samples is effectively the
observed maximum, not a strong estimate of rare tail behavior.

Keep compiler/version, Git revision and dirty diff, source/content hashes, and
machine/background activity alongside each run. Compare the same machine and
frozen asset library. A fixture hash alone does not freeze external instrument
code or sample files.

## Separate quantized check

`app::launch_benchmark::saved_project_quantized_launch_latency` measures request
preparation and control mirroring separately and advances the scheduler to a
sixteenth-note boundary. It audits the prepared snapshot's composed rack content
before accepting timing data. It is secondary to the default immediate path;
failed correctness checks must be resolved before treating its output as a
boundary performance baseline.

On 2026-10-12 this audit failed on `gotham-riddim-edit2`: target scene index 20,
track 0 had a composed rack pattern but the preflight snapshot marked it silent.
`eseq-38o3` tracks the fix. `preflight_pattern_launch_snapshot` reads the raw scene
cells instead of composing the rack clip lanes. The immediate path passed the
same content checks. `quantized-check.json` remains incomplete by design; its
timings must not be used for regression comparison.

## Gotham baseline, 2026-10-12

Apple M1 Max, 32 GiB, macOS 26.5.1, Rust 1.93.1, optimized release library test
compiled with nextest against the working tree based on
`7146dd1cb4826a09a40ddb8244c2675b83ca19c3`. The fixture has 18 tracks, 38 scenes,
59 Jungle-ology clips, 53 Gotham Piano clips and two published neural sequencers,
at 169 BPM. No fallback samples were loaded.

Input SHA256: `95dc7378238c35533f1c6a8a1cf0c27d51dc9cacd74c5beaa5d4eb709e68cbdd`.
Executable SHA256: `ccc92f2b3f568a467dbc599fe8c6bde75129772ee947d2211548c54a4d83c603`.

Three independent processes (`baseline-a`, `baseline-b`, `baseline-c`) each
passed 300 launch checks, with identical scheduling frontiers and 1,193 emitted
events per run. The table combines those 900 launches, first visits and revisits:

| Immediate control work | Median | p95 | p99 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| Full scene (228 launches) | 35.90 ms | 48.98 ms | 55.31 ms | 70.06 ms |
| Jungle-ology clip (354) | 33.36 ms | 40.29 ms | 49.29 ms | 87.83 ms |
| Gotham Piano clip (318) | 34.53 ms | 40.23 ms | 42.45 ms | 271.94 ms |

The separate scheduler refill p95 was 3.01 / 3.37 / 3.59 ms respectively;
the worst refill across all three categories was 4.54 ms. Full-scene first-visit
p95 ranged 49.89–55.31 ms across runs, versus 34.79–38.76 ms on revisits.

The 271.94 ms spike was round 0, piano rack clip 33: 6.62 ms assignment and
265.32 ms in `apply_manual_pattern_launch`. The identical case took 31.22 and
37.82 ms total in the other two runs. Its cause is **unattributed**; `eseq-nacy`
tracks CPU/wall-time profiling and actual live scheduler/audio deadline checks.

These are desktop-load baselines, not isolated-machine performance ceilings.
No compiler/test jobs were observed at the recorded baseline boundaries, but
the user's live `metal_seq`, Ableton, OBS and desktop apps remained running.
The a→b comparison flags 15 of 72 checks at the default tolerance despite an
identical executable. Do not call a small difference a regression without
repetition, or dismiss the large spike without investigating it.

Evidence lives in `.local/benchmarks/scene-launch-gotham-2026-10-12/`: the frozen
fixture, raw reports/logs, `overview.json`, executable, environment, Git diff,
source hashes and before/after process inventories. Production sources changed
concurrently after compilation (principally unrelated UI files); the baseline
therefore pins a copy of the completed executable and runs its exact ignored
test directly, using the stack budget read from `.cargo/config.toml`. No source
or content hashes changed during the three baseline measurements. Use nextest
for rebuilding/running future candidates as shown above, and preserve the
resulting executable and runtime content identity with each report.

The earlier `smoke` run passed another 300 launches but overlapped other work;
it is retained as validation, excluded from the table. The comparison helper
was checked against identical reports, an injected maximum-latency regression,
and incomplete/different-fixture/different-event-workload reports.


## Compact device-lock storage, 2026-10-12

`eseq-i1l6` replaces stored device lock matrices with `StepLockRows`: empty
snapshots allocate nothing, populated rows support direct indexed reads, and
clones share immutable storage until edited. Descriptor binding retains identity
metadata only for actual locks. Sparse project input stays compact throughout
loading. Snapshot restore clears omitted rows/cells, and clearing live locks
visits occupied steps rather than sweeping every device cell. Existing producer
ownership of playback snapshots is unchanged.

The same Gotham fixture still contains 1,517 patches and 30,971 stored device
slots (28,230 empty). Their lock values and identities now occupy 1,903,120 bytes
across 60 shared grids, versus the original live inspection's 3,289,824,128 bytes.
The counter includes the Arc/Vec allocations once each; it excludes allocator
size-class overhead, history, live atomic slots, and unrelated subsystems. No
patterns or independent sound entities were deleted.

Three optimized candidate processes passed the same 900 immediate launches,
including identical scheduling frontiers and 1,193 events per process. Pooled
results, compared with the original three baseline processes:

| Immediate control work | Baseline median / p95 | Compact median / p95 |
| --- | ---: | ---: |
| Full scene | 35.90 / 48.98 ms | 7.09 / 7.61 ms |
| Jungle-ology clip | 33.36 / 40.29 ms | 8.27 / 8.76 ms |
| Gotham Piano clip | 34.53 / 40.23 ms | 8.34 / 8.78 ms |

Candidate scheduler-refill p95 was 1.06 / 1.01 / 1.46 ms respectively, with a
1.81 ms maximum across all categories. All nine baseline/candidate pairings
passed the comparator (648 checks, zero alarms). These are desktop measurements;
background activity varied, so they do not establish live audio deadline safety.

Separate memory runs used the pinned old and new benchmark executables, in
sequence, with `vmmap -summary` after project setup. Their timing samples are
excluded from latency comparisons. Both completed all 300 launches:

| Headless process measurement | Before | After |
| --- | ---: | ---: |
| Physical footprint after setup | 6.1 GiB | 757.5 MiB |
| Physical footprint peak at probe | 10.9 GiB | 1.4 GiB |
| Maximum RSS over entire run (`wait4`) | 11,545,722,880 bytes | 1,570,357,248 bytes |

The UI is absent from both processes. An existing interactive process keeps its
old code and allocations until restarted; these numbers are not a direct
Activity Monitor measurement of the rebuilt GUI. The empty-app baseline and
live scheduler deadline/tail investigation remain separate work. Quantized
preflight remains blocked by `eseq-38o3` as described above.

Evidence and reproducible probe scripts are under
`.local/benchmarks/memory-compact-gotham-2026-10-12/`: `candidate-{a,b,c}.json`,
`latency-summary.json`, `latency-comparisons.json`, `baseline-memory.*`,
`candidate-memory.*`, source hashes, frozen executables, and focused test logs.
The final focused release validation passed 127 tests plus three bus-history
checks. All Rust library and binary test targets also passed `cargo check`.

An additional adjacent, unprofiled before/after pair (300 launches each) checked
for the lighter background load during candidate measurements. Full-scene p95
was 41.02 → 7.43 ms; Jungle-ology 29.26 → 8.99 ms; Gotham Piano 29.88 → 8.99 ms.
All 72 comparator checks passed with identical event counts and frontiers. See
`matched-before.json`, `matched-after.json`, and `matched-comparison.json` in the
same evidence directory. Runtime source/content hashes remained unchanged during
candidate measurement. The original baseline and candidate binaries have their
build/measurement manifests recorded separately; both executables loaded the
same current runtime Lisp and assets for the adjacent comparison.
