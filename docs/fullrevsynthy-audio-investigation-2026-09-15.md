# fullrevsynthy intermittent audio overruns — 2026-09-15

**The reported steady-playback glitch has not been reproduced or explained.**
The saved scene has substantial measured headroom. The user verified that the
harness recording sounds correct, and reported no glitch after restarting the
normal app. That narrows the investigation, but does not prove a restart fixes
the problem or that the original session had the same runtime state.

Tracked in **eseq-zfrg**, which remains open/in progress. No DSP or scheduling
fix was made, and no recorder was added to the app.

## Workload and headless measurements

Project snapshot SHA-256:
`3f848a135fbe66fb9a8ba855e1fdea3517871393017cb3f372c4caaeb738fd66`.
One scene, 120 BPM, 48 kHz stereo, actual device callbacks verified at 512 frames.
The save has Revsynt, an empty MIDI track, a modulator, and Digi Drift. Revsynt
feeds Slowdown, Filter Table and Str8 Delay; the project also has bus effects.
There is no project scratch source requiring UI authoring to construct playback.

Three live CoreAudio runs used the production scheduler and callback, four DSP
helpers, 20 seconds of warmup and 60 seconds of measurement each. Device output
was silenced after DSP. The original interactive app was closed and this task
ran no builds during these intervals. Desktop background activity remained.

| Pooled metric | Result |
|---|---:|
| Captured blocks / audio duration | 16,876 / 180.011 seconds |
| Mean callback | 0.837 ms |
| Median callback | 0.674 ms |
| p99 callback | 2.929 ms |
| Maximum callback | 3.655 ms |
| Block budget | 10.667 ms |
| Callback budget misses | 0 |
| New late / dropped scheduled events | 0 / 0 |
| Rust allocations / reallocations / frees on callback and helpers | 0 / 0 / 0 |

All runs verified 4/4 helpers in the device workgroup, including observer
refreshes and stable membership throughout measurement. The heap audit was
calibrated before each run. Its zero result covers executed Rust allocator paths,
not all native allocations or unexecuted interactions.

Revsynt rendered one voice per block; Digi Drift rendered six. Thus track count
alone understates DSP work, but the measured work still fits comfortably.
Timing includes application callback phases, but excludes the harness's later
signal-statistics scan and device silencing. It is not a hardware glitch detector.

## Listenable validation and graph samples

The user requested audio evidence before trusting the harness. The opt-in
experiment now accepts `--record-audio` (JSON config `output_wav`). It copies
output before device silencing into a bounded, preallocated queue, and writes
32-bit float WAV only after stopping the engine. Overflow or disagreement with
the captured block sample count fails the run. Samples are not normalized or
clipped; an existing output file is never overwritten.

The live recording contains exactly **1,536,000 stereo frames / 32 seconds**,
starting at scene playback with no warmup. The user listened and confirmed it
sounds correct. PCM SHA-256:
`235c28ed12b9605f8e1b82bf5a1156e3f137c747b999648531d3f8949bc310e0`.
The WAV data chunk was independently hashed and its format/duration verified.
The same run passed the Rust heap audit and workgroup verification. It had three
initial late scheduled events and no callback budget miss.

Forty graph slices were collected alongside that recording. The observed final
dependency chain was modulator → Revsynt → Slowdown → Filter Table → Str8 Delay.
Six Digi Drift voice kernels contribute much of the aggregate work. Representative
graph wall time was 2.731 ms, aggregate kernel time 4.417 ms, and longest chain
by kernel duration about 0.953 ms. Ready-to-start gaps were substantial, but
include dispatch and available-worker delays; they do not prove a scheduler bug.

The recording/tracing run's callback average was about 2.74 ms, higher than the
unrecorded baseline. Recording and tracing were enabled together and warmup was
different, so these data do not isolate the reason. Do not use this run as a
performance comparison or infer that recording overhead explains the difference.

## Normal app and external OS trace

The normal `target/release/metal_seq` binary was reopened unchanged (its build
timestamp remained September 15, 17:26). The user loaded fullrevsynthy and started
playback with the UI present. The app logged verified 4/4 workgroup membership.
The user reported **no glitch since restarting**. No relevant inherited audio or
profiling overrides were found in the earlier launch shell; that does not recover
every environment change made after the shell itself started.

`xctrace` Audio System Trace successfully attached. A requested 60-second capture
used the template's default five-second window; its exported data covered only
7.992 seconds / 749 application I/O cycles. Those cycles had mean 1.018 ms,
median 0.833 ms, p99 3.141 ms, and maximum 3.385 ms, all classified Normal. The
application audio points-of-interest table was empty. This is **not** evidence
of 60 seconds of measured clean callbacks.

A second capture requested 120 seconds with a 30-second window. Its finalization
expanded heavily, nearly exhausting available disk space. The profiler was
stopped; its incomplete trace and task-owned temporary stores were removed.
Closed raw `.ktrace` scratch from all three captures was also removed; the
completed pilot/first trace and extracted data remain. Free space recovered to
about 19 GB. The aborted trace provides no timing verdict. Avoid repeating this
long-window configuration on this host without budgeting its temporary storage.

## Validation and continuation

Two exact release nextest tests passed:

- `audio::experiment::pcm::tests::capture_preserves_partial_packets_and_unclipped_float_samples`
  checks partial packets, multiple channels, exact float samples outside [-1, 1],
  and zero heap operations while capturing.
- `audio::experiment::pcm::tests::overflow_and_length_mismatch_fail_the_capture`
  checks explicit rejection of overflow and incomplete output.

The release experiment build, Python syntax check, and scoped whitespace check
passed. Changes are limited to the optional experiment PCM output, its CLI and
documentation. No app callback recorder, performance workaround, commit or push.

The next useful reproduction is the app session **while it is actually exhibiting
the glitch, before restarting it**. Capture its device callbacks and scheduling
with the existing external tools, and compare runtime voice/graph state to this
verified snapshot. Current evidence points away from sustained overload in the
saved scene; it does not identify the cause of the original intermittent event.

Raw evidence lives under `.local/benchmarks/fullrevsynthy-2026-09-15/`: frozen
binaries/project/source, `baseline/`, `baseline-analysis.json`, `listen/`, graph
captures and analysis, completed OS traces, exported cycle data and test logs.

Reproduce a listenable live run:

```sh
cargo build --release -p sequencer --features audio-heap-audit --bin audio_experiment
python3 tools/audio-experiments/run.py \
  --project .local/projects/fullrevsynthy.json --pattern 1 \
  --names workgroup-on-w4 --warmup-bars 0 --measure-bars 16 \
  --record-audio --out /tmp/fullrevsynthy-listen
```

For timing, omit `--record-audio` and graph tracing, use `--warmup-bars 10
--measure-bars 30 --repeat 3`, and close the interactive audio engine first.
