# PM Piano performance follow-up — 2026-09-12

The second pass reduces native piano CPU by **24.7–25.5% versus v0.1.21**, while
retaining all 256 struck/reverse modes and every calibration coefficient. The
saved `pianotestperformance` project improves by **10.2% process CPU and 11.0%
mean callback time**. These are different measurements: piano work is only
part of the complete project's CPU cost.

## Measured results

Apple Silicon macOS, 48 kHz; one voice, seven alternating paired repetitions,
four seconds of audio each. Compilation, allocation and Python are outside the
native ABI timing loop. All candidates compile before timing starts.

| Patch / call size | v0.1.21 | Candidate | CPU reduction |
| --- | ---: | ---: | ---: |
| Normal / 128 | 153.93 µs | 115.95 µs | 24.7% |
| Reverse / 128 | 154.92 µs | 115.36 µs | 25.5% |
| Saved project / 128 | 153.06 µs | 114.90 µs | 24.9% |
| Normal / 512 | 614.52 µs | 459.61 µs | 25.2% |
| Reverse / 512 | 615.28 µs | 459.39 µs | 25.3% |
| Saved project / 512 | 616.74 µs | 461.47 µs | 25.2% |

The full-project comparison uses the same release `audio_experiment` binary,
project snapshot, device, 512-frame buffers and four workers on both sides.
Each run warms up for 8 seconds and measures 16 seconds. Two runs per side in
ABBA order reduce drift; there are no concurrent builds or DSP benchmarks.
The baseline loads an isolated copy of the old piano source. The original
saved project is not edited. Device output is silenced during measurement.

| Full project | v0.1.21 mean | Candidate mean | Reduction |
| --- | ---: | ---: | ---: |
| Process CPU | 66.80% | 59.99% | 10.2% |
| Callback wall time / RT budget | 22.60% | 20.12% | 11.0% |
| Per-run callback p99 / RT budget | 44.26% | 39.71% | 10.3% |

All four runs had zero over-budget callbacks, measured late events, dropped
events and non-finite samples. These are local host measurements, not a promise
of exact Activity Monitor readings with the interactive UI running.

Raw timing repetitions, audio errors, host summaries and artifact identities
are in [`performance-followup.json`](../tools/pm-piano/performance-followup.json).
The [first-pass report](pm-piano-performance-2026-09-12.md) remains unchanged.

## What changed

1. **Coefficient scheduling:** `piano-control` uses `event-hold` on the existing
   16-sample clock. Pure dependent coefficient calculations run on that clock;
   final audio-rate latches still supply the recurrence. Control cadence and
   maximum latency remain 16 and 15 samples respectively.
2. **Reverse carrier normalization:** a shared scale `1.5 - 0.5*(x*x+y*y)`
   replaces one square root and two divisions per mode per sample. This is a
   Newton correction for an oscillator initialized on the unit circle, not an
   approximation applied to general square roots or damped string amplitudes.
   For squared norm q in [0.5, 1.5], q*(1.5-0.5*q)^2 lies in [0.78125, 1].
   Even a 1% rotation-magnitude error leaves the broader interval invariant.
   Positive common scaling preserves oscillator phase.
3. **Compiler scratch allocation:** tensors whose producers and every reader
   belong to one mandatory sequential frame group use one frame of scratch.
   The schedule completes all group fragments for frame i before advancing to
   i+1, so earlier frames are no longer live. Cross-loop readers, materialized
   host values/views, hop/event scatter, training and Metal retain their prior
   policy. SIMD widths and fusion decisions are unchanged; less unnecessary
   memory traffic benefits the existing vector loops.
4. **Patch persistence:** both edited piano macros are regenerated through the
   patcher's source-promotion API. Root graph and root geometry are unchanged.
   The sidecar regression now compares local macro operator counts as well as
   parameter contracts and numeric tables, catching a stale algorithm even
   when its sound remains nearly identical.

A separate exploratory compiler-only comparison measured about 11.1% lower
piano CPU before applying the two DSP changes. The compiler optimization can
help other matching graphs; no universal percentage is established for other
instruments.

## Sound, correctness and costs

All 152 paired comparisons pass: ten presets, all 88 keys, automation,
retriggers, project settings, 44.1/48/96 kHz, 32/128/512-frame compilations,
and a long bass swell. Maximum normalized RMS error is **0.003681%**; maximum
absolute sample delta is **7.53e-6**. The numerical change is very small, but
this is not a blind listening test or a claim of bit-identical audio.

The existing complete piano verifier passes 338 checks; the swell verifier
passes 112 renders, including a two-minute held bass at 96 kHz. Coverage includes
all controls, extremes, tuning, envelopes, release/pedal behavior, live blend,
modulation and irregular processing calls. The final regenerated sidecar is
separately rendered with the same streaming cases: normal and reverse sample
deltas are 4.49e-6 and 8.94e-6, within the existing 1e-5 tolerance. Those targeted
results refresh the corresponding entries in the checked-in validation reports.

Twelve targeted compiler tests pass across `EventHoldTests`,
`SequentialTensorScratchTests` and `TensorHistoryHopGatingTests`. Six other
factory instruments (Cello, Bonang, Crash, Saron, Clarinet and Ride) pass paired
old/new compiler audio comparisons at 32 and 128 frames. The raw audition path
cannot directly compile Flute's library imports; Flute is instead checked through
the production host, which expands those imports. Production `instrument_probe`
passes for normal, mixed and reverse piano, and Flute.

**RAM tradeoff:** event scheduling conservatively retains more coefficient
intermediates between separate loops. At 128 frames, per-voice state changes
from 79,520 to 217,233 floats: about 0.53 MiB extra. At 512 frames it changes
from 293,024 to 843,921 floats: about 2.10 MiB extra. This is allocated instrument
state, not allocation inside the audio callback. The compiler change reduces
scratch within each proven group but does not erase required cross-group tapes.

There is no mode pruning, voice-limit reduction, early tail termination, or
approximation of the general math library. No known fragile workaround remains;
the audible approximation is specifically the bounded unit-carrier correction
above, and RAM is the measured implementation tradeoff.

## Distribution and reproduction

DGen compiler source: `7cef5f81b08097523613c03665fc12fa61d1b9aa`.
The macOS archive is published as
[DGenLisp v0.1.22](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.22).
Its SHA256 is `30825a961453f39e18c0b9fd234bd7a75ce32e04212a274084e068b9558d5ce7`.
The clean-worktree release uses Apple Swift 6.2.3 / Xcode 26.2, strip and ad-hoc
signing. Scalar and grouped-parameter/modulation smokes verify the standalone
ABI and bundled audit resources. The packaged and fetched compiler's generated
piano C must match the benchmarked candidate byte for byte. Linux remains at
v0.1.20; these measurements establish no Linux or Metal performance claim.

The original compiler distribution and piano source must be retained separately
for a paired rerun. With that v0.1.21 compiler at `$BASELINE_COMPILER`, run:

```sh
git show '9e7982f5:content/instruments/Physical Models/PM Piano/dsp.lisp' > /tmp/piano-v21.lisp
python tools/pm-piano/performance.py \
  --baseline-compiler "$BASELINE_COMPILER" \
  --baseline-source /tmp/piano-v21.lisp \
  --compiler crates/sequencer/tools/DGenLisp-macos-arm64 \
  --output /tmp/piano-followup
```

Add `--params /path/to/project-params.json` for a saved parameter map. Use the
piano README's verifier and sidecar commands for the broader sound checks.
Reload the project to replace already compiled voices with the new DSP.
