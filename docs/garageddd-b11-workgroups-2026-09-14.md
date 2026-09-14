# garageddd B11: macOS device workgroups

All four DSP helpers now join the actual output AudioUnit's device workgroup.
In six warmed live captures, enabling membership reduced aggregate p99 callback
time from **5.918 to 3.777 ms**, and the observed maximum from **7.461 to
3.969 ms**. Mean time increased slightly, from **2.964 to 3.048 ms**. Both
configurations had zero measured budget misses; these results support improved
tails for this workload, not a claim that the earlier rare glitch is eliminated.

## Implementation and ownership

CPAL 0.15.3 already uses AudioUnit on macOS. The pinned local distribution in
`vendor/cpal-0.15.3` adds an owned workgroup-property getter and a `Send` bound
on its internal property listener. It exposes no private-layout casts or raw
AudioUnit handle. The exact local changes are documented in `ESEQ-PATCH.md`.

`audio/workgroup.rs` observes the active stream every 100 ms on a control
thread. It checks actual group identity, publishes changes to the native engine,
and records each helper's join result. CoreAudio property reads, reference
releases, acknowledgement waits, and membership logging stay off audio threads.
Helpers only perform their join/leave operation and atomic acknowledgements.

The native engine retains a group until every helper acknowledges departure.
Replacement and clearing wait on the control thread without the old unsafe
timeout/release behavior. The stream wrapper pauses output and joins the
observer before worker-pool destruction. Failed property reads and joins remain
visible in control-thread diagnostics; experiments reject unverified membership.

Apple documents the device workgroup as the means of relating parallel audio
work to the device deadline in [Adding parallel real-time threads to audio
workgroups](https://developer.apple.com/documentation/audiotoolbox/adding-parallel-real-time-threads-to-audio-workgroups).
The installed AudioUnitProperties.h documents that reading
`kAudioOutputUnitProperty_OSWorkgroup` returns a +1 reference owned by the caller.

## Capture conditions

- Apple M1 Max, macOS Darwin 25.5.0; live CoreAudio, 48 kHz, stereo, 512 frames.
  Each block has a 10.666667 ms budget. Headless DSP runs normally; only the
  samples sent to the output device are silenced.
- Frozen garageddd snapshot, bank B scene 11 (one-based scene 23), 156 BPM.
  The current user project had changed, so both configurations used the earlier
  measured snapshot. Project SHA-256:
  `091e93675a262046b7606c389899ecc03ba87ae5ace250fd35b1c5edc9270c41`.
- Three processes per configuration, each with 20 seconds of warmup and 60
  seconds of measurement. Fixed shuffled order: off-r0, on-r2, on-r1, on-r0,
  off-r2, off-r1. Both configurations use the same four-worker wait policy,
  property observer, and calibrated Rust heap instrumentation.
- No builds were run by this task during capture. The user's desktop session
  was left intact; this is not an isolated hardware laboratory measurement.
- The same executable was frozen before capture. SHA-256:
  `08f821ad481fdf4fd977f24c2312a65a518e334d3079bb6dd9d69abace8acbd4`.
  Base commit: `336b288e982202fbfdf71bd4f1a4a66af873e3e5`, plus recorded working
  changes. DGenLisp macOS distribution: v0.1.24. All 14 instrument engine source
  hashes and configured voice capacities match across all six processes.

## Results

Percentiles below are calculated from pooled blocks, not averaged percentiles.

| Membership | Blocks | Mean | Median | p99 | Worst | Budget misses |
|---|---:|---:|---:|---:|---:|---:|
| Disabled | 16,876 | 2.964 ms | 2.877 ms | 5.918 ms | 7.461 ms | 0 |
| Enabled, 4/4 helpers | 16,876 | 3.048 ms | 3.137 ms | 3.777 ms | 3.969 ms | 0 |

| Run | Mean | p99 | Worst | Process CPU |
|---|---:|---:|---:|---:|
| off-r0 | 2.804 ms | 5.960 ms | 6.698 ms | 103.6% |
| off-r1 | 2.990 ms | 6.056 ms | 7.209 ms | 127.4% |
| off-r2 | 3.097 ms | 3.651 ms | 7.461 ms | 133.2% |
| on-r0 | 3.352 ms | 3.810 ms | 3.969 ms | 151.8% |
| on-r1 | 2.671 ms | 3.500 ms | 3.934 ms | 120.7% |
| on-r2 | 3.121 ms | 3.347 ms | 3.543 ms | 111.2% |

Aggregate process CPU was 121.4% disabled and 127.9% enabled, on the scale where
one fully occupied core is 100%. Workgroups did not reduce average CPU cost in
this capture. Per-run variation and the short interval limit causal claims.

The worst disabled block spent 7.423 of its 7.461 ms in graph rendering, with
no dispatched event. The worst enabled block spent 3.901 of 3.969 ms in graph
rendering. These are elapsed times, including descheduling and helper waits;
they do not identify a specific kernel or prove the cause of the old 11.669 ms
outlier. Both sides include the earlier slow-render warning removal.

## Verification and limits

- All six runs verified the expected membership at warmup completion and after
  measurement: 4/4 joined when enabled, 0/4 when disabled. All reported zero
  join failures, property errors, membership changes, or verification failures;
  observer refresh counts increased throughout each interval.
- All six calibrated Rust audits counted zero allocations, reallocations, and
  deallocations in the callback and all four helper threads. This is exact
  counting of exercised Rust allocator paths, not proof about unexecuted code
  or native allocations. Capture and allocator scopes have separate boundary
  atomics, so their callback counts can differ by one at interval edges.
- All six had zero measured late or dropped scheduled events and no nonfinite
  output samples. RMS ranged 0.088501–0.088524, peaks 0.531465–0.532912.
  off-r1 had 11 late events before measurement; its measured delta was zero.
- Native lifecycle tests passed 20 cycles for each of 0, 1, and 4 workers:
  joining, repeated identity, replacement after releasing external ownership,
  cancelled-group `EINVAL`, clearing, shutdown while assigned, and restart.
  The existing scheduler-completion regression also passed.
- The targeted ignored Rust test opened a silent real CoreAudio stream and
  verified four joins, repeated source reads, and cleared membership after
  teardown. It passed with the heap-audit feature.
- The normal `cargo build --release -p sequencer --bin metal_seq` completed
  successfully after the captures, with workgroups enabled by default.
- Physical device switching was not performed. Changes are adopted on the next
  successful 100 ms observer refresh, subject to thread scheduling. Workgroups
  do not guarantee deadlines.
- Experiment callback timing excludes its subsequent signal-statistics scan
  and device silencing. It is not an acoustic or hardware-underrun detector.
  The normal transport meter measures the enclosing device callback.

An existing issue, **eseq-2pqj**, still causes native `apply_params` to print
out-of-range parameter warnings from the render path. Those warnings occur in
both these and the earlier captures, independently of scheduled-event counters.
That producer defect and real-time I/O require separate correction; membership
does not fix them. **eseq-cczw** remains open because the original rare render
deadline miss has not been conclusively attributed.

Raw evidence, configs, logs, source snapshots, executable, and `analysis.json`
are retained in `.local/benchmarks/garageddd-b11-workgroups-2026-09-14/`.
Reproduction commands and test selection are documented in
[the experiment guide](../tools/audio-experiments/README.md#macos-device-workgroup-comparison).
