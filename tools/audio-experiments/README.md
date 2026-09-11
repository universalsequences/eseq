# Audio scheduler experiments

This opt-in harness measures a saved scene through the production CoreAudio
stream, callback, scheduler, graph workers, instruments and effects. It loads a
private project snapshot, runs a musical warmup, and measures an unchanged scene.
The output device receives silence **after** DSP and signal measurements. It
does not open the UI or save changes to the project.

Normal builds have no experiment hooks or scheduler overrides. On macOS the
shipping callback policy now spins 64 times before a completion-aware wait of
up to 50 microseconds; workers retain eight spins and a 50-microsecond wait. Build the
separate executable explicitly:

```sh
cargo build --release -p sequencer --features audio-experiments --bin audio_experiment
python3 tools/audio-experiments/run.py \
  --project .local/projects/phsyicsxdf.json --pattern 4 \
  --out /tmp/audio-sweep --repeat 3
```

Stop other audio playback and builds before measuring. Scene numbers are
one-based, matching the UI. By default, each process warms up for four bars and
measures eight bars. The output directory must be new. It retains a project
snapshot, config and logs for every run, per-block raw metrics, and a summary.
The run order is shuffled with a fixed seed. `--binary /path/to/audio_experiment`
selects a build; `--baseline-binary /path/to/old/audio_experiment` interleaves a
saved baseline for the variants in `--baseline-names` (default
`workers-4,workers-6`). Metadata records each binary's SHA-256.

Use `--names workers-4,workers-6,queue-hint-w4` for a subset. `--offline` uses the
strict synchronous export driver instead of CoreAudio and includes an audio
SHA-256 for deterministic output comparisons. Offline throughput does not
predict live scheduling latency; live mode is the performance comparison.

## What the numbers mean

* `process_cpu_pct`: aggregate process CPU seconds / measured wall seconds,
  on the same one-core-equals-100% scale as Activity Monitor. The UI is absent.
* `callback_*_pct`: callback render time / frame budget, summarized as mean,
  p50, p95, p99 and maximum. `over_budget_blocks` counts render budget misses;
  it is not a hardware/acoustic glitch detector.
* `cpu_pct_per_audio_second`: CPU cost normalized to frames actually rendered,
  useful when the zero-worker configuration falls behind real time.
* `measured_late_events` and `measured_dropped_events`: change in cumulative
  callback counters between the first and last captured blocks. The unprefixed
  counters also include startup/warmup, which can have separate late events.
* `peak`, `rms` and the finite-sample check guard against benchmarking silence
  or invalid output. Offline hashes provide the stronger correctness check.

The callback metric is taken before the experiment's signal-statistics scan;
that scan and the final device silencing still count toward process CPU.
Initialization, compilation, warmup and shutdown are outside both measured
intervals. Each process owns exactly one C engine. Stream teardown precedes
App resource destruction, and workers stop before the graph is freed.

## Policies and correctness checks

The `workers-N` control sweep uses 0, 1, 2, 3, 4 and 6 helpers, holds realtime
scheduling enabled, and explicitly uses the old spinning callback policy.
`callback-wait-50-w4` and `callback-wait-50-w6` exercise the new macOS default. Other candidates vary worker polling effort and optionally
check the queue length before attempting an MPMC pop. This queue length is an
advisory hint, never a proof that a concurrent producer has finished publishing.

```sh
python3 tools/audio-experiments/check_scheduler.py
```

This compiles queue-saturation, block-event, ordered-summing and completion/lifetime
regressions in a disposable directory and runs them against eight policies. It
does not modify shared Make artifacts. `--normal` checks the shipping policy
without experiment hooks; `--diagnostics` enables scheduler diagnostic bookkeeping;
`--tests` selects a comma-separated subset of the four binaries.

The initial experiments exposed graph-lifetime and completion-wake failures in
`worker-spins-1`, longer worker waits and `callback-wait-*`. The completion-aware
queue and session reference barrier fix those failures; reproduction cases are
retained by name. The default sweep remains a small set of controls; select wait
variants explicitly when comparing their tradeoffs.

See [the initial results](../../docs/audio-worker-experiments-2026-09-11.md) and
[the completion fix and follow-up measurements](../../docs/audio-idle-wait-fix-2026-09-11.md).
