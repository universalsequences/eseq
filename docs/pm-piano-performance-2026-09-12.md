# PM Piano performance, 2026-09-12

The unchanged PM Piano costs 46–47% less CPU with two DGen C renderer fixes.
Paired CoreAudio runs of `pianotestperformance` use 33.6% less process CPU and
32.6% less average callback time. The fixes are published and installed as
[DGenLisp v0.1.21](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.21)
for macOS Apple Silicon. Linux remains independently pinned to v0.1.20.

## Measurements

| Measurement | Pinned compiler | Local candidate | Reduction |
| --- | ---: | ---: | ---: |
| Piano, saved patch, 128 frames | 290.93 µs | 153.59 µs | 47.2% |
| Piano, saved patch, 512 frames | 1152.98 µs | 618.97 µs | 46.3% |
| Project process CPU | 97.46% | 64.75% | 33.6% |
| Project mean callback / RT budget | 32.24% | 21.72% | 32.6% |
| Mean of project runs' p99 callback / RT budget | 69.07% | 44.01% | 36.3% |

Single-voice measurements call the compiled native process ABI directly, without
Python, allocations, compilation, or I/O in the timed region. Each case uses
seven alternating paired repetitions, four seconds of audio per repetition,
48 kHz, and a one-second warmup. Values are medians. Default and fully reversed
patches show similar reductions.

Project measurements use the production `audio_experiment` CoreAudio host,
four workers, 48 kHz, 512-frame device blocks, eight seconds of warmup and
sixteen seconds of measurement per run. Order: baseline, candidate, candidate,
baseline. Values average two runs per compiler. Output was silenced; the UI was
absent. Every run had zero measured late events, dropped events, and over-budget
callbacks. These runs reproduce the saved project, but do not measure the
interactive UI's CPU. CPU percentages therefore need not match Activity Monitor
while the UI is open.

## Cause and implementation

The banks already use tensors. They contain 128 struck modes (64 fast and two
32-mode slow banks), plus 128 reverse carriers per voice. Six overlapping voices
make the original roughly 11%-of-one-core voice cost consistent with the user's
roughly 65-point process CPU increase.

The compiler was obstructing SIMD and hardware math:

1. Integer loop counters lost their emitted integer type. Tensor and tape reads
   consequently performed floating-point `isfinite` checks on integer indices.
   The runtime's finite classifier intentionally contains an optimization barrier,
   which prevented LLVM from vectorizing these independent lanes. Preserve the
   loop counter's type and omit tape finite checks only for proven integer indices.
   Floating checks and tape bounds remain. This removed 274 unnecessary checks
   from the piano and increased vectorized loops from eight to twenty in the
   integer-fix comparison.
2. The freestanding C toolchain treated ordinary elementary libm calls as opaque.
   Emit explicit Clang builtins for scalar min, max, abs, sqrt, floor, ceil and
   round, including existing modulo and square-root power specializations.
   This allows hardware lowering under the existing numerical policy.

Both changes are in `Sources/DGen/Renderer/CRenderer.swift` in the adjacent dgen
checkout. No instrument source, preset, bank count, tail behavior, or approximation
was changed. There is no piano-specific compiler path.

Recurrence prevents independent evaluation of successive samples, but independent
modal states can still occupy separate NEON lanes. This fixes demonstrated
obstacles; it does not establish that every remaining loop achieves maximal SIMD
utilization. Halving modes was experimentally faster but changed some bass
waveforms materially, so that experiment was not adopted.

## Validation and limits

- 152 waveform comparisons: ten presets at five pitches, all 88 piano keys,
  automation at three sample rates and three block sizes, a 32-second bass swell
  with pedal, and four notes using the saved project parameters.
- Maximum absolute sample difference: `6.2883e-6`. Worst normalized RMS waveform
  error: `2.8925e-5` (0.0029%). This is numerical equivalence testing, not a claim
  of bit identity or a completed blind listening study.
- One-sample and irregular stream partitions (1, 7, 12, 63, 128) pass against the
  baseline, with finite state and maximum sample difference `1.714e-7`.
- Four production `instrument_probe` checks pass: three piano configurations and
  the allpass synth. The release compiler and release host probe binaries build.
- Five new compiler tests pass (integer/float index contracts, scalar builtin
  emission, signed math through feedback across block partitions). Twelve existing
  event-hold, gather and tensor-history hop tests also pass in targeted runs.

The measured whole-project reduction is about one third, below a literal 40%
CPU reduction target; the piano itself is close to half cost. Remaining project
CPU includes effects, scheduling and host work. Other platforms were not measured.
The renderer changes apply beyond piano; targeted tests and the additional synth
probe do not substitute for platform release validation. No fragile workaround is
known in the implementation.

## Reproduction and activation

`tools/pm-piano/performance.py` reproduces the waveform and native-kernel comparison:

```sh
.local/venvs/physical-models/bin/python tools/pm-piano/performance.py \
  --baseline-compiler .local/benchmarks/piano-2026-09-12/baseline-v0.1.20/DGenLisp \
  --compiler crates/sequencer/tools/DGenLisp-macos-arm64 \
  --params .local/benchmarks/piano-2026-09-12/project-params.json \
  --output .local/benchmarks/piano-2026-09-12/recheck
```

The original timings selected the development compiler through
`ESEQ_DGENLISP_TOOL`. The published compiler now loads through the normal installed
path with no overrides. Restart the app and reopen the project to recreate its
instrument instances. Compiler fingerprints invalidate the old compiled cache;
manual cache deletion is unnecessary.

The release was built in a clean checkout of dgen commit
`53bdba8ad997a84f86b19dbd3cdb1ae168e33f17`, stripped, ad-hoc signed, and packaged
with runtime headers, ABI allowlists and the binary-audit script. Archive SHA-256:
`9be84624d043469e275ffa577e90113028759e95989d0bcd13ee8d909be767e5`.
An anonymous public download matched before the macOS pin was updated and
`scripts/fetch_dgenlisp.sh --target macos-arm64 --force` installed it.

Both the packaged compiler and the installed symlink passed scalar and
grouped-parameter/modulation compilation with resource overrides removed.
Their generated PM Piano C is byte-identical to the benchmarked candidate
(`e070808466f11726ea6e9cb88a268a5aabca9c2b9a1ec87c35427dd5ff7fc002`).
The installed compiler also passed the production host piano probe: 96,000 frames,
48 kHz, 512-frame blocks, MIDI 50, peak 0.33385, RMS 0.05133, no non-finite audio
or state. These release checks are in the sibling `release/` evidence directory.

Tracked summary: `tools/pm-piano/performance.json`. Local detailed evidence:
`.local/benchmarks/piano-2026-09-12/final/`, including `results.json`,
`live-results.json`, per-run JSON/logs, `host-probes.json`, `streaming.json`,
`provenance.json`, and the compiler diff. The project snapshot, configuration,
and named parameters are in the parent directory. The summary records hashes
because both repositories also contain working-tree changes.

Tracking: dgen-1lq and dgen-qnp cover the compiler fixes, eseq-efcc covers
publication and installation, and eseq-pwn5 retains the broader instrument
performance work, including cello.
