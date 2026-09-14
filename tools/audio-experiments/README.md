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

## Native real-time safety audit

On macOS, run the calibrated audit separately from timing measurements:

```sh
python3 tools/audio-experiments/audit.py \
  --project .local/projects/garageddd.json --pattern 23 \
  --scope all --seconds 60 --repeat 3 --out /tmp/garageddd-rt-audit
```

`--scope all` checks the entire application-owned CPAL callback from its first
entry, shared offline rendering, and native DSP worker lifetimes, including
startup, project loading and teardown. `--scope measured` explicitly excludes
loading and the configured warmup (20 seconds by default). A callback already
in flight completes its scope; workers update their scope at block wakeups.
Use both scopes when distinguishing steady playback from lifecycle violations.
The default production build contains none of these sanitizer hooks.

The script builds the separate `audio-rtsan` feature with `RTSAN_ENABLE=1` and
uses the pinned `rtsan-standalone` 0.3.0 integration. Building that feature without
`RTSAN_ENABLE=1`, or on an unsupported target, fails instead of silently disabling
checks. The dependency obtains the pinned upstream runtime distribution on its
first build; the runner retains its binary and hash alongside the executables.

Before playback, separate-process controls must detect Rust allocation/free and
native `malloc`, `calloc`, `realloc`, `posix_memalign` and `free` from a library
loaded with `dlopen`, just like a compiled instrument. A stack-only control must
remain clean. Failed basic calibration or missing sanitizer completion stats
invalidates the run. No suppressions are inherited from the environment.

**Coverage limit:** the current runtime misses direct macOS
`malloc_zone_malloc`/`malloc_zone_free` calls. The runner deliberately exercises
those too and records the gap; it never labels a run a zero-allocation
certificate. Custom allocators and unexecuted branches also require separate
coverage. Positive violations are conclusive; absence of reports is bounded by
the tested workload and verified interceptors. This is native call interception,
not allocation sampling and not just Rust's global allocator.

Exit status 1 means observed real-time violations, 2 means no observed violations
but incomplete allocator coverage, and 0 requires a clean run and all calibration
controls passing. The retained `summary.json` distinguishes unique heap reports
from total real-time errors, which also include locks and I/O. Sanitizer logging
and stack collection perturb execution, so its callback timings must not be used
as performance results. It does not open or alter an existing app session.

## What the numbers mean

* `process_cpu_pct`: aggregate process CPU seconds / measured wall seconds,
  on the same one-core-equals-100% scale as Activity Monitor. The UI is absent.
* `callback_*_pct`: whole-callback elapsed time / frame budget, summarized as mean,
  p50, p95, p99 and maximum. `over_budget_blocks` counts callback budget misses;
  it is not a hardware/acoustic glitch detector.
* `blocks[].phases`: wall-clock microseconds spent in snapshot/transport sync,
  pool sync, live input, control parameters, scheduled events, voice retirement,
  graph rendering, and post-render work. These contiguous phase timings locate
  callback stalls that the render-only terminal warning misses. They include
  time when the callback thread is descheduled, so a slow phase alone does not
  distinguish computation from waiting. Phase capture is compiled only with
  `audio-experiments` and adds no callback allocation or logging.
* `blocks[].events`: separates countdown collection, scheduled-queue draining,
  and event dispatch, with the dispatched count and slowest individual event.
  `slowest_event` includes its kind, zero-based track and step, frame offset,
  and wall time (including destruction of its owned payload). It is null when
  no event was dispatched. Event timings have the same descheduling caveat as
  the phase timings; they are not CPU-time profiles.
* `cpu_pct_per_audio_second`: CPU cost normalized to frames actually rendered,
  useful when the zero-worker configuration falls behind real time.
* `measured_late_events` and `measured_dropped_events`: change in cumulative
  callback counters between the first and last captured blocks. The unprefixed
  counters also include startup/warmup, which can have separate late events.
* `peak`, `rms` and the finite-sample check guard against benchmarking silence
  or invalid output. Offline hashes provide the stronger correctness check.
* `instrument_voice_stats`: engine names, source hashes, configured capacity,
  enabled voices at measurement end, and process/voice-zero call counts. The
  counters reset after warmup. Live reads are approximate at the interval
  boundaries; `process_calls / voice_zero_calls` estimates voices rendered per
  block, including release tails, rather than currently held notes.

Check workload parity before using these results for projects with Lisp-authored
graph sequencers. The harness does not evaluate project scratch on the UI
authoring runtime. In `mow` B2, the missing graph publication left rack engines
rendering one voice while the normal app reached twelve, even after 45 seconds
of warmup. Those headless CPU numbers do not represent that project's playback.
This limitation is tracked in `eseq-tdjl`; see
[the live investigation](../../docs/mow-performance-2026-09-13.md).

The callback metric is taken before the experiment's signal-statistics scan;
that scan and the final device silencing still count toward process CPU.
Initialization, compilation, warmup and shutdown are outside both measured
intervals. Each process owns exactly one C engine. Stream teardown precedes
App resource destruction, and workers stop before the graph is freed.

## Headless FilterTable compiler comparison

`compare_filter_table.py` compiles the unchanged production causal FilterTable
with two compiler binaries and loads both through the native DGen ABI, using
the app's Accelerate FFT host services. It requires macOS and NumPy. For a local
compiler build, supply its audit script through `DGEN_BINARY_AUDIT_TOOL`:

```sh
DGEN_BINARY_AUDIT_TOOL=/path/to/dgen/scripts/audit-dgen-dylib.sh \
  python3 tools/audio-experiments/compare_filter_table.py \
  --baseline crates/sequencer/tools/DGenLisp-macos-arm64 \
  --candidate /path/to/dgen/.build/release/DGenLisp \
  --out /tmp/filter-table-comparison
```

The comparison covers flat/shaped procedural magnitude banks, static controls,
frame/cutoff/resonance sweeps, and regular/irregular process calls. Cross-compiler
waveform differences above the stated tolerance fail the run. Existing differences
between partitions are reported separately (`eseq-mi4l` / `dgen-j6r`); a passing
compiler comparison does not certify partition invariance. Audio arrays, compiler
hashes, generated C, manifests and results remain in the output directory.

Timing excludes Python and uses native thread CPU time, with 256 warmup blocks,
2,048 measured blocks and nine alternating repetitions after two discarded rounds.
Stop other benchmarks/builds for the final run. This measures one effect's DSP
cost, without voice allocation, graph scheduling, the UI, or the saved project's
particular magnitude bank. It does not estimate four-worker transport CPU.

## Steady playback Rust heap audit

`audio-heap-audit` wraps Rust's global allocator and marks the complete CPAL
callback, shared offline renderer, and native DSP helper threads. It counts
`alloc`, `alloc_zeroed`, `realloc`, and `dealloc`; C allocations are outside this
specific audit. Setup and warmup are excluded. Scheduler/UI allocations are
excluded by thread scope, even when they overlap playback.

Every experiment first calibrates both callback and helper roles with explicit
allocation, zeroed allocation, reallocation and free, plus unmarked work that
must be ignored. Calibration failure aborts. The JSON includes separate callback
and helper counters, callback entries, actual helper thread entries, and
`rust_heap_audit_passed`. A nonzero counter or missing thread coverage exits
unsuccessfully after preserving the JSON result. No allocator interception or
counter updates remain in normal builds.

For garageddd B11 at its saved 156 BPM, 13 warmup bars are 20 seconds and 39
measured bars are 60 seconds:

```sh
cargo build --release -p sequencer --features audio-heap-audit --bin audio_experiment
python3 tools/audio-experiments/run.py \
  --project .local/projects/garageddd.json --pattern 23 \
  --names callback-wait-50-w4 --warmup-bars 13 --measure-bars 39 --repeat 3 \
  --out /tmp/garageddd-rust-heap-audit
```

This is exact counting of executed Rust heap operations within the measured
audio-thread scopes. A clean run establishes zero for that workload and interval;
it does not prove unexecuted scenes, live edits, loading, or native allocation
paths. Use the native sanitizer audit above when stacks or C coverage are needed.

## macOS device workgroup comparison

Normal macOS builds bind all DSP helpers to the active output AudioUnit's
workgroup. CPAL already uses AudioUnit; the pinned local CPAL extension exposes
only the owned workgroup property. A control thread reads that property every
100 milliseconds, adopts changes, and reports actual helper join results.
Property reads, reference releases, waiting for acknowledgements, and logging
stay outside audio threads. The native engine retains the previous group until
every helper acknowledges departure. Stream teardown stops and joins the
observer before the worker pool can be destroyed.

Use the same heap-audit binary for both sides of the comparison:

```sh
cargo build --release -p sequencer --features audio-heap-audit --bin audio_experiment
python3 tools/audio-experiments/run.py \
  --project .local/projects/garageddd.json --pattern 23 \
  --names workgroup-on-w4,workgroup-off-w4 \
  --warmup-bars 13 --measure-bars 39 --repeat 3 \
  --out /tmp/garageddd-workgroups
```

These variants hold the shipping four-worker wait policy constant and vary
only helper membership. Both run the property observer, so its control-thread
overhead is present on both sides. The JSON includes `workgroup_start`,
`workgroup_end`, and `workgroup_verified`. A live macOS run fails verification
if helpers fail to join, the expected worker count is wrong, property reads
fail, the observer stops refreshing, or the source workgroup changes during
measurement. Disabled runs must verify zero joined helpers. Offline and
non-macOS runs report null for this verification.

The native lifecycle regression covers join, same-group reads, replacement,
cancelled-group failures, clearing, and pool restart. The ignored macOS test
opens a silent real output stream and checks membership, observer refreshes,
and teardown:

```sh
python3 tools/audio-experiments/check_scheduler.py --normal \
  --tests test_workgroup_lifecycle,test_scheduler_completion
cargo nextest run -p sequencer --lib --features audio-heap-audit \
  -E 'test(=audio::workgroup::tests::macos_output_stream_verifies_helpers_and_releases_membership)' \
  --run-ignored all --no-capture
```

Actual device changes are adopted on the next successful observer refresh;
these tests do not switch the system output device. Workgroup membership gives
macOS information about related real-time work, but is not a deadline guarantee.
Compare averages and tails separately, and retain the measured interval and
number of blocks when reporting zero misses.

## Parallel DSP timing in the app

For projects that require UI authoring, the normal app can capture individual
graph slices with the same opt-in feature:

```sh
cargo build --release -p sequencer --features audio-experiments --bin metal_seq
TINYSEQ_AUDIOGRAPH_PROFILE=/tmp/mow-profile.jsonl target/release/metal_seq
```

The output path must not already exist. Load the project, start the desired
scene, and allow its voices and effect tails to warm up. From another terminal:

```sh
touch /tmp/mow-profile.jsonl.request
```

The recorder consumes the request file and captures forty slices over about
twenty seconds. Creating another request after capture finishes appends another
set. File access, allocation, and JSON writing happen on a control thread. The
callback and helpers fill a bounded snapshot; metadata is copied after the
timed graph interval. Normal builds omit this instrumentation entirely.

```sh
python3 tools/audio-experiments/analyze_profile.py /tmp/mow-profile.jsonl \
  --out /tmp/mow-analysis.json
python3 -m unittest discover -s tools/audio-experiments -p test_analyze_profile.py
python3 tools/audio-experiments/check_scheduler.py --tests test_graph_profile
```

The analysis separates aggregate kernel time, per-worker busy time, and graph
wall time. It reports the longest dependency chain by kernel duration and the
predecessor chain that actually finished last, with ready-to-start gaps shown
separately. Those gaps include binding, dispatch, and waiting for an available
worker. They are not evidence of a scheduler defect by themselves. A companion
`.trace.json` file uses the Chrome/Perfetto trace format; slot zero is the callback
and positive slots are helper workers.

These are graph slices, which can be shorter than a complete audio callback
when events split the block. Captures also add timing overhead and are sampled
diagnostics, not a substitute for a warmed callback CPU comparison with tracing
disabled. Do not interpret summed kernel time as transport CPU when workers
execute concurrently. Captures exceeding 4,096 nodes or 32,768 edges are marked
incomplete and rejected by the analyzer.

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
`--tests` selects a comma-separated subset of the correctness binaries.

The initial experiments exposed graph-lifetime and completion-wake failures in
`worker-spins-1`, longer worker waits and `callback-wait-*`. The completion-aware
queue and session reference barrier fix those failures; reproduction cases are
retained by name. The default sweep remains a small set of controls; select wait
variants explicitly when comparing their tradeoffs.

See [the initial results](../../docs/audio-worker-experiments-2026-09-11.md) and
[the completion fix and follow-up measurements](../../docs/audio-idle-wait-fix-2026-09-11.md).
