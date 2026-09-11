# Physics project: CPU and audio deadline experiments

Date: 2026-09-11. Task: eseq-dvy5.

This is the historical pre-fix report. See the [completion fix and follow-up
measurements](audio-idle-wait-fix-2026-09-11.md) for subsequent work.

**Keep the existing waiting policy.** None of the tested small scheduler changes
provided a repeatable CPU saving while preserving callback performance and
reliable shutdown. Six workers buy 3.0 percentage points of callback headroom
for 10.0 additional process CPU points compared with four. More aggressive
waiting exposed synchronization/lifecycle failures, so it cannot be treated as
an available optimization.

## Worker count

| Configuration | Runs | Process CPU | Mean callback budget | p99 callback budget |
|---|---:|---:|---:|---:|
| workers-0 | 1 | 57.6% | 103.6% | 130.8% |
| workers-1 | 1 | 114.6% | 58.7% | 61.1% |
| workers-2 | 1 | 121.5% | 46.6% | 47.6% |
| workers-3 | 1 | 124.8% | 40.5% | 41.7% |
| workers-4 | 3 | 134.6% | 37.5% | 39.7% |
| workers-6 | 3 | 144.6% | 34.5% | 36.3% |

Values are medians across runs where repeated; p99 is the median of each run's
p99, not a pooled percentile. Four workers measured
134.4–135.3% process CPU;
six measured 144.5–148.2%.
Single-run entries are useful screening observations, not precise estimates of
small differences.

Every successful nonzero-worker run stayed within the callback budget and had
zero newly observed late or dropped events between its first and last measured
blocks. Some runs had startup/warmup late events, retained separately in the
raw cumulative counters.

**Zero workers is not an efficiency win.** All 596 captured blocks
exceeded the render budget. It produced 6.36 seconds of audio
in 13.43 wall seconds—only 47.3% of real-time throughput.
Its low Activity Monitor-style percentage therefore describes substantially
less audio work per second. It does not establish that the same functioning
playback can use 58% CPU.

## Other policies

| Configuration | Runs | Process CPU | Mean callback budget | p99 callback budget |
|---|---:|---:|---:|---:|
| workers-4 | 3 | 134.6% | 37.5% | 39.7% |
| queue-hint-w4 | 3 | 134.6% | 37.7% | 39.8% |
| worker-spins-64 | 2 | 135.8% | 37.9% | 40.0% |
| worker-spins-1024 | 1 | 141.5% | 37.7% | 39.2% |

* **Queue hint:** check the advisory queue length before attempting an MPMC pop.
  At four workers, three runs were effectively neutral in process CPU and
  slightly slower in average callback time. Two- and six-worker screening runs
  also showed no convincing gain. A cheaper busy loop still occupies a CPU
  while waiting for the DSP; these measurements do not measure energy/power.
* **Worker spinning:** increasing the current eight attempts to 64 did not
  improve the tradeoff. Increasing to 1,024 added about seven CPU points with
  no useful average callback improvement.
* **One worker spin:** the C regressions passed, but the live run finished its
  measurement and then hung joining a worker. LLDB showed `runFlag=0`,
  `workSession=NULL`, and the worker still in the queue's semaphore wait.
  Disqualified; eseq-j67n.
* **Callback parking:** a six-worker / 10-microsecond wait run hung with
  `jobsInFlight=0`, queue length zero, and one callback waiter. All six helpers
  were asleep on their session condition variable. CoreAudio could not finish
  stopping the callback. Disqualified; eseq-j67n.
* **Longer worker waits:** the existing queue-saturation test trapped at graph
  destruction with a 1,000-microsecond worker timeout. The stack was
  `_dispatch_semaphore_dispose → rq_destroy → destroy_live_graph`.
  This exposes the distinction between completed DSP jobs and workers having
  left the graph's queue. Disqualified pending a lifetime fix; eseq-z75r.

The callback and worker hangs both reached the untimed wake-drain path in
libdispatch's semaphore wait. Apple's source explicitly permits this path
after a timeout races a signal; the observations do **not** establish why its
expected wake was absent here. Do not interpret the configured microsecond
timeout as a proven hard bound. [Apple semaphore implementation](https://github.com/apple-oss-distributions/libdispatch/blob/main/src/semaphore.c#L98-L126).

## Recommended next work

The next prerequisite for saving spinning CPU is a queue notification and
shutdown design that wakes on **new work, block completion, and shutdown**, and
keeps the queue alive until every worker has left it. A callback-parking policy
needs that foundation and deadline/tail-latency validation. Changing a timeout
or adding an unconditional sleep is not a validated fix.

For current playback, four workers remain a reasonable default; six provide
more deadline headroom at a measured CPU cost. Three reduce CPU but add callback
time. The best choice depends on whether power use or room for denser DSP is
more valuable. The tests do not establish a universal optimum for other scenes.

## Method and validation

* Apple M1 Max, MacBookPro18,4, ten physical/logical cores. The user closed
  `metal_seq`; normal desktop background processes remained. No builds or C
  tests ran concurrently with the accepted live measurement windows.
* Saved `phsyicsxdf`, visible scene/pattern **4** (internal index 3), 11 tracks,
  143 BPM, DGenLisp macOS v0.1.19. Each run loaded a byte-identical private
  snapshot. The real CoreAudio output was 48 kHz stereo, 512-frame graph blocks.
* **18 accepted live runs** on the final measurement revision, plus three
  successful setup/pilot runs. Each accepted run warmed up four bars
  (~6.71 seconds) and measured eight bars (~13.43 seconds). Candidate order was
  shuffled. Four/six workers and the four-worker queue hint received three runs;
  the 64-spin policy received two. Failure runs are not counted as successful
  performance observations.
* The benchmark uses the production CPAL stream, scheduler, callback, graph
  workers, DSP and scene-launch path. It silences only the final device output.
  There is no UI rendering. Process CPU is therefore lower than the full app's
  Activity Monitor reading and should be compared **between harness runs**.
* Process CPU is process CPU seconds divided by wall seconds. Callback budget
  is callback render duration divided by the duration of its frames. Signal
  measurement and device silencing occur after the callback measurement and
  count toward process CPU. Initialization, warmup and teardown are excluded.
* Five offline renders—zero workers, four workers, callback wait 50 µs,
  four-worker queue hint and 64 worker spins—matched bit-for-bit:
  `c07dd241fba0e5016e967c4df4bf89c45284681e2c5490f69d2084cecd25a253`. These used one warmup bar and two measured bars
  through the strict synchronous export driver. This verifies audio for those
  cases, not live wait/shutdown correctness.
* Fifteen focused native C checks passed: queue saturation, block-event
  timing, and ordered-sum topology across five policies. Normal (feature-off)
  C syntax validation also passed. The live wait failures show why these tests
  alone are insufficient to certify a scheduler change.
* Experimental controls and measurement hooks require the explicit
  `audio-experiments` Cargo feature. Ordinary app builds retain their existing
  behavior. Known failing policies are excluded from the default sweep and
  retained by explicit name for reproduction. No changes were committed.

## Reproduction and evidence

[Harness instructions](/Users/alecresende/code/learning/anthropic/eseq/tools/audio-experiments/README.md) ·
[Machine-readable results](/Users/alecresende/code/learning/anthropic/eseq/tools/audio-experiments/results/2026-09-11.json)

Raw project snapshots, configs, block metrics, source snapshot, binary hash,
logs and debugger traces are preserved locally under:
`/Users/alecresende/code/learning/anthropic/eseq/.local/benchmarks/audio-workers-2026-09-11`.

[Callback hang trace](/Users/alecresende/code/learning/anthropic/eseq/.local/benchmarks/audio-workers-2026-09-11/eseq-audio-experiment-lldb-queue.txt) ·
[Worker shutdown trace](/Users/alecresende/code/learning/anthropic/eseq/.local/benchmarks/audio-workers-2026-09-11/eseq-audio-worker-stop-lldb.txt) ·
[C regression results](/Users/alecresende/code/learning/anthropic/eseq/.local/benchmarks/audio-workers-2026-09-11/eseq-audio-scheduler-checks2.log)

The archived first-version block field `start_sample` contains the rendered
sample cursor at block end; the harness now names that field `rendered_samples`.
That labeling correction does not change any timing, CPU, or audio values.
