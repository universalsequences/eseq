# Modal Snare performance — September 14, 2026

The factory Modal Snare now uses **62–66% less native DSP time per voice** for
Jungle S. In the saved six-voice `modalsnaretest` project, the median audio
callback load fell **49.8%**, and process CPU fell **46.2%**. These measurements
use the exact final source and the same pinned compiler on an Apple Silicon Mac.

## Change

The original generated kernel recalculated large tables of decay, tip-spread
and brightness coefficients every sample, even when their effective inputs
were unchanged. The factory now uses the event scheduling support already used
by the gamelan and piano instruments: `event-hold` schedules pure coefficient
work, and final `latch` nodes return the results to audio-rate resonators.

Eight effective controls invalidate the per-voice cache: stretch, split, tilt,
visc, release, release2, tip and bright. Changes are detected every sample using
exact equality. First use and runtime sample-rate changes also invalidate it.
There is no periodic control clock, deadband or added modulation latency.

Both 72-slot head banks, the scalar head proxies, all 12 two-partial wires,
projection contact, rim, bend, pressure and six-voice setting are retained.
The coefficient equations, baked tables, parameters and presets are unchanged.
Continuously changing one of the cached controls still requires recomputation
on every changed sample; the static-preset savings do not predict that cost.

## Measurements

Native process-call timing, 48 kHz, one voice, Jungle S, velocity 1. Each entry
is the median of seven alternating paired repetitions, four seconds of audio
per repetition. Compilation, Python and allocation are outside timing.

| Frames | MIDI note | Before, µs/call | After, µs/call | Reduction |
| --- | --- | --- | --- | --- |
| 128 | 29 | 219.61 | 82.46 | 62.4% |
| 128 | 43 | 219.97 | 82.22 | 62.6% |
| 128 | 69 | 226.54 | 82.63 | 63.5% |
| 128 | 89 | 229.58 | 80.57 | 64.9% |
| 512 | 29 | 943.86 | 334.12 | 64.6% |
| 512 | 43 | 947.43 | 329.32 | 65.2% |
| 512 | 69 | 954.28 | 327.90 | 65.6% |
| 512 | 89 | 981.93 | 329.06 | 66.5% |

The project comparison uses the production `audio_experiment` host on CoreAudio
with silent output: scene index 1, 48 kHz, 512-frame callbacks, four workers,
eight seconds of warmup and sixteen seconds measured. Runs were interleaved
baseline/candidate/candidate/baseline. The existing interactive app stayed open;
there were no concurrent benchmark or build processes. The same host executable
was used on both sides. Figures below are medians of the two runs per side.

| Project measurement | Before | After | Reduction |
| --- | --- | --- | --- |
| Mean callback budget used | 22.46% | 11.28% | 49.8% |
| Per-run p99 callback budget used | 26.96% | 15.38% | 43.0% |
| Process CPU | 76.82% | 41.34% | 46.2% |

All four runs had zero measured late events, dropped events or callback budget
misses. Host instrumentation confirmed six enabled voices, approximately 9,000
voice process calls against 1,500 voice-zero calls, and the expected source
hashes. The engine reserves capacity for twelve voices; six were enabled and
rendering. The source project was untouched; snapshots redirected only the
instrument identifier to temporary copies of the two sources.

These are headless host measurements, not a reading of the interactive app's
reported 33% meter. UI work and scheduling variation affect that meter. Reload
the factory instrument/project to compile the updated source; no compiler or
app binary update is needed for this instrument change.

## Sound and correctness

All 80 two-second comparisons passed: default plus all 15 factory presets at
five pitches. Maximum overall level change was 0.129 dB, maximum first-100-ms
level change was 0.129 dB, and maximum change in any broad band's share of total
energy was 0.0282. Jungle S's level change stayed below 0.000085 dB and its
normalized waveform RMS difference stayed below 0.186%.

This is not bit-identical output or a perceptual equivalence claim. Moving pure
math changes floating-point scheduling; tiny coefficient rounding differences
can grow into different trajectories in nonlinear wire contact. Tight Piccolo
and deep sun snare show substantial waveform decorrelation despite close
level and broad spectral distributions. No voicing adjustment was made to
compensate for that difference.

Additional checks passed:

- All eight cache inputs changed on adjacent samples and returned to earlier
  values, including zero. The 17 observed coefficient outputs matched the old
  calculations within `2.3e-7` normalized error.
- Runtime sample-rate changes with retained state matched within `1.1e-7`.
- At 44.1, 48 and 96 kHz, every cached control was driven by audio-rate modulation.
  Gate changes, pitch changes and retriggers crossed irregular process calls of
  1, 7, 12, 63, 128, 3 and 512 frames. Candidate audio was sample-identical across
  those partitions versus regular 128-frame calls, and old/new signal gates passed.
- The host coefficient-event regression passed for voices 0 and 5, including
  first-use initialization, adjacent events, returns to zero, and 1/7/32-frame
  blocks. The existing rim key-tracking regression also passed.
- The factory Jungle S host probe rendered 96,000 frames with peak 0.508,
  RMS 0.0442, and no nonfinite audio or state. Generated-kernel fusion checks passed.

No approximate-math substitution, model reduction or compiler workaround was
introduced. A separate existing compiler issue was exposed by diagnostic
outputs with tightly allocated short buffers: some SIMD I/O loops extend past
the requested frame count. It is recorded as **eseq-4sdm**. The benchmark uses
the existing audition/host convention of fixed-capacity channel allocations;
the odd frame counts and partition comparisons above remain exercised. No
production change was made to conceal or compensate for that compiler issue.

## Evidence and reproduction

- [Runner and commands](../tools/modal-snare/README.md)
- [Native timing and validation data](../tools/modal-snare/performance.json)
- [Final project run summaries](../tools/modal-snare/project-performance.json)
- Local raw sources, project snapshots, configurations and full block logs:
  `.local/benchmarks/modal-snare-2026-09-14/`.

Baseline source SHA256:
`917148b1915b77123e31a0394fce100c229109b1bd8ef7ff453918d26e578e20`.
Final source SHA256:
`fefac9de4f71b6c1875687cdda63fe3b1467ef1248935998ecf25791f1deb1c8`.
Pinned compiler SHA256:
`f37c0c7c16575b5f6ffdadaeca94493dd5d5a37084321e6141dd08a5c9a7da8b`.
Host executable SHA256 is preserved with the project run summaries.

Beads: **eseq-a5v5** (optimization), **eseq-4sdm** (separate compiler follow-up).
