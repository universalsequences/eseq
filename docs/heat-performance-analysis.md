# Heat DSP performance investigation — 2026-09-07

The initial investigation below predates the delivered execution gates. The
implementation and final measurements are recorded at the end of this report.

The dominant cost is executing unused synthesis, especially the three disabled
unison copies. Diagnostic pruning reduces the default patch's DSP time by about
5.8× with the host's compilation settings, retaining host modulation support.
The initial stereo trace and subsequent release trace match exactly. This is
measured optimization headroom, **not an implemented runtime optimization** or
proof of parity with Ableton Analog.

## Measurement

Apple M1 Max, 48 kHz, default parameters, one held 220 Hz note at velocity 1.
Compiler: published macOS DGenLisp v0.1.10, upstream
`07ea090151c5b31eeffeb1d72caae49df4feaa75`; pinned hermetic toolchain.
Eseq HEAD: `e2c6f72f`; synthesis changes: `7129b827`, following `b24bc1f0`.
The current working-tree instrument has moved to `content/instruments/Synths/Heat`.

Each trial invokes `dgen_process_v1` from a native C loop, avoiding Python call
and buffer-copy overhead. Five trials of approximately two seconds of audio per
case; minimum time shown below, medians and every trial retained in
`tools/heat/measurements/dsp-profile-20260907.json`. Compiler maximum block size
is 512. The host compiles `--voices 12`; this allocates per-voice scratch and
uses a dynamic scratch offset, but **each call still renders one host voice**.

| Host-compatible build, default sound | µs / 128 frames | µs / 512 frames | One-core budget at 512 |
| --- | ---: | ---: | ---: |
| Current Heat, unison parameter = 1 | 290.8 | 1129.9 | 10.59% |
| Only the first complete unison copy exists | 93.7 | 371.5 | 3.48% |
| Only default-used sections exist, modulation retained | 50.0 | 196.4 | 1.84% |

The first change saves 67% at 512 frames; both together save 83%, or 5.75×.
This last diagnostic removes oscillator 2, noise, both disabled LFO signals,
the unused second output lane, hard-sync generation, unused main waveforms,
sub-oscillator output and bypassed drive. It retains the selected filter path,
envelopes, smoothing, and host modulation accessors. It is not a fully minimal
synth and leaves further mode-dependent work, such as the filter's second SVF
stage, for later investigation.

The diagnostic cannot replace Heat: removed functionality is unavailable if a
parameter changes. Its purpose is to establish what the default sound actually
needs to compute. No production DSP or compiler source was changed.

## Where the regression came from

`dsp.lisp` expands `heat-voice` four times (lines 343, 359, 375, 391).
Each expansion owns two oscillator paths, two filters, four envelopes and two
LFOs. The final output multiplies each copy by a smoothed enable value.
That multiplication silences a copy without suppressing its execution.

In the isolated `--voices 1` build, Unison 1 costs 254–257 µs/128 frames;
Unison 4 costs 256–257 µs. They are effectively the same workload. The older
`b24bc1f0` benchmark source costs 51.7 µs with the current compiler. The current
single-copy diagnostic costs 76–78 µs: newer within-copy features add cost too,
but four-way expansion is the largest regression.

## Compiler changes are helping

For current full Heat in the isolated build:

| Compiler configuration | µs / 128 frames | µs / 512 frames |
| --- | ---: | ---: |
| Current passes | 254–257 | 1025–1034 |
| DCE disabled alone | 254.6 | 1027.2 |
| DCE, static hoisting and coalescing disabled | 377.2 | 1575.4 |

DCE alone produces the same frame-loop and scratch-store counts here. Disabled
sections remain reachable through runtime selectors, so DCE cannot remove them.
The combined passes save roughly one third of current full-patch execution time.
The passes-disabled initial trace differs by at most `1.49e-8`; DCE-disabled
output is exact in the captured trace.

Generated C confirms the graph expansion: current full Heat has 260 frame-loop
sites and about 1.13 MB of C, compared with 68 loops / 333 KB for one copy and
34 loops / 178 KB for the default-used diagnostic. These are source counts,
not instruction counts or direct memory-bandwidth measurements. The C contains
both scalar feedback loops and NEON loops; history values cross loop boundaries
through scratch arrays. Hard-sync calculations and unselected waveform work
are emitted before output selection. The modulation loop computes all four
input/depth products before selecting the base parameter when modulation is
inactive (e.g. isolated one-copy `patch.c`, lines 2970–2990).

## Remaining single-copy costs

These independent source ablations use `--voices 1` and 128-frame blocks.
They overlap through shared dependencies and compiler scheduling, so **do not
add the savings**. Removing filters/envelopes changes the sound; those cases
attribute cost rather than demonstrate safe optimizations.

| Diagnostic | µs / block | Reduction from 78.3 µs baseline |
| --- | ---: | ---: |
| One complete copy | 78.3 | — |
| Bypass both filter/drive paths | 57.8 | 26% |
| Replace all host `(mod p)` reads with base `p` | 62.9 | 20% |
| Remove hard-sync computation | 68.6 | 12% |
| Replace four voice envelope outputs with 1 | 68.5 | 12% |
| Replace both LFO outputs with 0 | 72.0 | 8% |

For the default-used diagnostic, removing modulation changes 43–44 µs to
33.8 µs. This includes downstream work becoming frame-invariant, not merely
four multiply-adds per destination. Retaining modulatable parameters is
compatible with the large 5.8× improvement; removing modulation is not the
recommendation. A future inactive-modulation fast path should preserve the
active audio-rate path and make block-invariant downstream evaluation possible.

## Recommended implementation order

1. Implement explicit conditional execution in dgen (`eseq-xeq4`), and apply it
   to Heat's unused unison copies (`eseq-vumc.13`). This is the largest measured
   opportunity. Define state ownership and freeze/reset/re-enable behavior;
   do not silently change existing `gswitch` semantics.
2. Apply that mechanism within each copy: hard sync, oscillator enable,
   waveforms, unused lanes and mode-specific filter/drive stages. Routing and
   shared consumers matter: oscillator 2 being disabled does not prove filter 2
   is unused, because filter 1 or other sources may feed it.
3. Profile inactive modulation and scalar recurrence execution after gating.
   Consider static/dynamic specialization and then voice SIMD only if the
   remaining measurements justify the compiler/host complexity.

Smoothed enables require explicit treatment: a formerly active copy must render
through its fade and intended release. A naive branch on the raw enable can
truncate audio. Frozen LFO/filter state and canceled delayed unison onsets also
need deliberate semantics and toggle/retrigger tests. Diagnostic pruning proves
no such transition behavior.

## Host meter and validation limits

The transport meter is a smoothed ratio of entire callback wall time to the
buffer's time budget (`audio/callback.rs`, around line 891), including host
work and worker waits. These benchmarks measure generated DSP only. Three
serially rendered current voices would consume about 32% of one core; worker
parallelism and other callback costs prevent treating this as a reconstruction
of the user's 30% reading. Held-note count and live callback traces were not
captured. No instrumented comparison with Ableton was performed.

If 30% is predominantly this DSP and scales similarly, a 5.75× DSP reduction
suggests roughly 5.2% for that component. That is a projection, not an app result
or guarantee of matching Analog. Buffer size changes little in these tests:
per-sample DSP dominates call overhead.

The host retains released voices for up to 20 seconds and uses a contiguous
voice-count cutoff. The user confirmed this is intentional worst-case release
handling. It remains unchanged and is not proposed as an optimization target.

Validation completed:

- Production `target/debug/instrument_probe content/instruments/Synths/Heat
  --frames 48000 --gate-frames 24000 --sample-rate 48000 --block-size 512
  --min-peak 0.01 --min-rms 0.001 --json`: peak 0.0464, RMS 0.0145,
  no non-finite output or state. This uses the existing probe binary, not a
  freshly rebuilt host.
- Initial stereo traces (31 blocks) and one-second release traces are bit-exact
  between full, one-copy and default-used diagnostics at both block sizes,
  including the host-compatible 12-voice builds. Final release block is zero.
- Generated-C fusion checker passes for full and default-used builds.
- No whole-package tests, app relaunch, sound edits or compiler edits.

All generated sources, C, dylibs, traces, compilation logs and the one-off
profiling script are retained in `/tmp/heat-profile-20260907/`. The script uses
this checkout and cached builds; use a fresh output directory when changing
source/compiler/flags. Raw timing results, source hashes and validation metrics
are retained in the repository JSON above. Scratch ablations are measurement
experiments, not production-ready implementations.


## Delivered execution gating

DGenLisp v0.1.11, commit `28073ed`, is published and pinned for macOS arm64.
Both repositories use `codex/heat-execution-gating` in their existing checkouts;
no worktrees were created for this implementation. The compiler now provides
explicit C scalar execution regions and automatic unassigned-modulation gating.
Heat gates unused complete unison voices, sources, LFOs, waveform/sync branches,
optional second SVF stages and drive. Shared predicate work remains unconditional.
Filter/amp routing is not pruned using a default-preset assumption.

Nine interleaved native trials, alternating baseline/candidate order, measured:

| Default, one host voice | Original median | Gated median | Speedup |
| --- | ---: | ---: | ---: |
| 128 frames | 281.4 µs | 49.4 µs | 5.69× |
| 512 frames | 1086.8 µs | 164.6 µs | 6.60× |

The 512-frame DSP cost is about 1.54% of one core, versus 10.19% before.
This exceeds the requested 4× target for default DSP; live app-meter and Analog
comparisons have not been measured. The generated C, binaries, hashes and all
trials are described in `tools/heat/measurements/execution-gating-20260907.json`.
The reusable `tools/heat/profile_execution.py` checks equivalent compiled pairs.
Baseline artifacts remain in `/tmp/heat-profile-20260907/full-host12`; release
candidate artifacts remain in `/tmp/heat-gating/published`. Recreate the original
source from eseq commit `e2c6f72f` (the old `tools/heat/instrument/dsp.lisp` path),
resolve its macros and host preamble, and compile with v0.1.10. Compile the
current resolved source with v0.1.11; use the same pinned native stage, 48 kHz,
12 voices and 512 maximum frames for both.

The default and fractional-waveform comparisons are bit-exact at both sizes.
Four-voice maximum error is 7.45e-9; all-on error is 6.04e-7. Every comparison
releases to exact zero. Integrated source/unison toggle sequences remain finite
and finish with exact zero tails. The existing 60 voice integration checks and
15 unison cases pass, including presets and 44.1/48/96 kHz filter extremes.
The compiler's 27 selected execution-gate, modulation and optimization tests pass.

State semantics are deliberate: disabled DSP freezes and resumes, rather than
silently running. Audio-rate gate state advancement is whole-call, with output
masking per sample; resumed phase may depend on block boundaries. Heat uses
finite 2 ms enable fades so disabling completes its fade before execution stops.
Continuous control smoothing and the host's 20-second release retention remain.
Automatic inactive-modulation gating skips the combiner; specializing the whole
downstream graph to parameter rate remains future work. The Linux compiler
release is also outstanding; its pin remains unchanged. No compatibility
fallback or hidden default-preset specialization was introduced.

Production validation after downloading the release: the existing
`target/debug/instrument_probe` rendered the default and a two-second sequence
of mid-note unison/oscillator changes, with zero non-finite audio/state slots.
Default peak/RMS were 0.0464/0.0145; toggled peak/RMS were 0.0642/0.0161.
Adding/removing a 5 Hz volume modulation assignment while rendering also matches
the original compiler exactly at 128/512 frames and audibly changes the output.
Full validation records are in
`tools/heat/measurements/execution-validation-20260907.json`.
Follow-ups: dgen-7tx (downstream rate specialization), dgen-684 (Linux release).


## v0.1.12 correction after Vox click report

The initial validation missed a delay-buffer SIMD addressing regression exposed
by splitting execution regions. Vox (the synth in the user's “80s guitar” sound)
contains no explicit block-gate, but automatic modulation lowering changes its
loop layout. A frame-varying offset materialized into a global buffer was treated
as scalar because the last scalar load updated renderer type bookkeeping.
The resulting contiguous SIMD read crossed the circular delay boundary and read
the delay counter as audio. At 512 frames, reproduced spikes reached 43.12;
the old compiler peaked at 0.178. This also reproduced without LFO input.

Dgen commit `5458b54`, published and pinned as v0.1.12, classifies materialized
frame offsets by their lane behavior for reads, writes and accumulation. It
uses each lane's address; it does not disable modulation/gating or conceal the
spikes with a limiter. A three-line delay regression fails on v0.1.11 with
38.2 maximum error and passes after the fix at startup and ring wrap.

The exact saved Vox parameters with zero or four smooth LFO inputs now agree
with v0.1.10 within 4.34e-7 at both 128/512 frames. All four simultaneous
modulation sources, with distinct values/depths, are also checked explicitly in
all three modes and 1/12-voice compilation. Twenty-nine focused tests passed.
Raw Vox comparisons are in `tools/heat/measurements/vox-gating-regression-20260907.json`.
These are isolated renders, not a live capture of the user's LFO configuration;
live listening confirmation remains necessary for the original audible report.

Heat's v0.1.12 paired benchmark remains 5.69×/6.65× faster than the original
at 128/512 frames, with default audio exact. Raw trials are retained separately
in `tools/heat/measurements/execution-gating-v12-20260907.json`.

Post-install production `instrument_probe` with Vox and pan_width=0.4 rendered
two seconds at 48 kHz/512 frames: v0.1.11 peak 43.1065, v0.1.12 peak 0.159293,
zero non-finite audio/state. The release archive was retrieved via GitHub's API
after the normal asset URL returned HTTP504, SHA256 checked against the pin,
and installed in the standard distribution layout. `fetch_dgenlisp.sh` confirms
that the installation matches v0.1.12. Existing loaded instruments must be
reloaded; compiler-content hashing gives recompiled DSP a new cache key.
