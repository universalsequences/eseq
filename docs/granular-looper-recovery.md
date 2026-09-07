# Granular Looper: binary-recovery findings

Research date: 2026-09-06. Tracking: `eseq-rlwn`; compiler prerequisite:
`eseq-a1v4`.

## Local listening version installed

The user clarified that a cool, playable local effect is the goal; exact
reference matching is not a deployment requirement for this experiment.
The listening version is now installed at
`.local/effects/granular-looper/dsp.lisp`, alongside four planar curve JSON assets
and a README. This is gitignored local content, not a factory release.

It uses fixed-capacity stereo tensors: 1,161,600 recording frames and 1,048,576
delay frames, with runtime sample-rate timing and bounded periods/extensions.
Approximately 3-second playback spans plus overlap fit through 192 kHz; above
that the period is capped to physical capacity. This is a valid buffer design,
not a requirement for another compiler feature. Defaults are 3 heads, 350 ms,
2.5 Hz, 15% overdub, and 30% dry. No timing offsets were added to mask discrepancies.

The installed version passed finite/nonzero wet-render checks at 44.1/48/96/192
kHz, parameter response, silence, freeze/trigger transitions, a 192 kHz ring-wrap
stress with maximum length/heads/overdub, rapid contraction, bounded operation at
384 kHz, and exact audible-output equality for blocks 1/64/512. Compilation used
the pinned v0.1.8 tool and its inline audit. These are offline ABI checks, not
an interactive-app or exact-reference sign-off.

A paired control test caught an inactive trigger. Explicit `seq` dependencies
now preserve previous-sample gate, initialization, target, and DC-input snapshots
before replacing them. Trigger changes now affect the rendered signal. Upstream
history-ordering investigation is tracked separately as `eseq-q75r`.
Local-audition task: `eseq-54v1`. Exact-reference findings below are historical
research, not a promise that the listening version is an exact clone.

## Compiler prerequisite resolved

DGenLisp [v0.1.8](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.8)
now supplies `poke` and `seq`. The macOS arm64 compiler was published from
`a2d037c6779924e8753e3c311060e5f93b6e7aae`, re-pinned in eseq, and installed
through the normal SHA-verified fetcher. Its generated counter fixture produced
exactly samples 1..1024 across fixed and mixed host block sizes through the
installed symlink. The clean upstream checkout passed 23 focused tests.
Linux's compiler pin is unchanged and does not yet supply these operators.

The syntax is `(poke tensor index value)` or `(poke tensor index channel value)`;
`(seq first second ...)` returns the last scalar. Shared bindings remain
snapshots. Mutable buffer tensor math/views are explicitly rejected, and
mutable-buffer gradients are not promised. See the upstream README for the
complete supported contract. The failure probes below record the **old**
compiler; they are historical evidence, not the current macOS status.

This is an incomplete reverse-engineering record, **not a claim of sonic
equivalence**. A local listening implementation is available as described above. The experimental implementation name is Granular
Looper. Only the installed plug-in binary was analyzed; the author's Max patches
were not opened. Subsequent validation runs the plug-in headlessly through
Pedalboard using its normal VST3 interface. No protection changes or asset
redistribution were performed.

## Provenance and method

Authorized reference executable:

```
/Library/Audio/Plug-Ins/VST3/helisert_6.1.vst3/Contents/MacOS/helisert_6.1
```

Universal-binary SHA-256:
`498c2afbe79dac4ae14b81c8b210471ff89848c7e597e02ac62a9becf83cf229`.

Analysis used `file`, `nm -arch arm64`, `c++filt`, `otool -L`,
`strings -arch arm64`, and `xcrun llvm-objdump --macho --arch=arm64 -d`.
Addresses below are ARM64 unslid virtual addresses, not universal-file offsets.
The executable retains C++ symbols and uses JUCE. The core is inlined into
`PluginProcessor::processBlock` at `0x2ea50`; preparation is at `0x277554`.

The Apple objdump invocation used here did not honor start/stop address filters;
filter the disassembly by function labels instead. Temporary function listings
from this session are in `/tmp/granular-looper-analysis.7esH2x` (not durable).

## Verified instruction-level structure

- The playback-head loop ends at index 15 (`0x301ec–0x301f4`). A separate
  comparison against a stored count controls per-head activation, with smoothed
  weights (`0x301f8–0x30294`). This is a capacity of 15 slots, not proof that
  all 15 are always audible.
- Each active head adds an index-dependent phase offset to a common phase and
  wraps modulo one (`0x30298–0x302c4`). The denominator's complete parameter
  mapping is not yet established.
- Buffer reads perform adjacent-sample linear interpolation
  `a + fraction * (b - a)` (`0x30344–0x30390`). The second index wraps to zero
  at the physical buffer boundary. Active loop length and physical buffer
  capacity must not be conflated in a translation.
- Near a boundary, another path blends the main buffer with interpolated delay
  reads, using table coefficients (`0x30398–0x304cc`). Delay indices use masks.
- A second read position, displaced by a loop-length quantity, is crossfaded
  into the first (`0x304d0–0x30580`). This is not simply one Hann envelope
  multiplied by each independent grain.
- Head parity changes summation signs (`0x301bc–0x301d4`). Do not replace this
  with ordinary positive-only summation or assume it is conventional panning.
- Summed head weights enter a power-law gain expression with exponent 0.4
  (`0x3058c–0x305f0`). Another factor involves a square root and constants
  approximately 0.725 and 0.275. The source of that factor is not yet mapped.
- The stereo output includes a recurrence of the form
  `y[n] = x[n] - x[n-1] + r*y[n-1]` (`0x305fc–0x30634`), consistent with DC
  blocking. Feedback-delay writes follow (`0x30638–0x30694`).
- A transition branch uses the JUCE-style 48-bit LCG multiplier `0x5deece66d`,
  increment 11, followed by absolute-value and a 0.9 power curve
  (`0x2fdfc–0x2fe80`). Its complete trigger/skid semantics remain unresolved.
- Preparation allocates a stereo main buffer of approximately 6 seconds plus
  a 50 ms margin, and prepares two delay lines for 5 seconds each. These are
  allocation observations, not recovered public parameter limits.

## Crossfade tables: mathematical identification

The constructor loads four embedded PCM16 WAV resources into audio buffers
(`0x3228c–0x322f0`). Initially they were decoded in memory to inspect the numeric
curves. The subsequent local validation candidate uses temporary JSON tables
extracted from that same authorized binary; no WAVs or table arrays are included
in this repository document or distributed.

| Symbol | ARM64 resource address | Dimensions | Observation |
| --- | --- | --- | --- |
| `SinCrossfader_wav` | `0x32c0d4` | 48000 frames, 2 channels | complementary raised-cosine ramps |
| `mixXfader_wav` | `0x35af3e` | 48000 frames, 2 channels | sine/cosine ramps with approximately 1.3 exponent |
| `pNormCurveUp_wav` | `0x389da8` | 50 frames, 300 channels | bank of rising curves; exact generating formula unresolved |
| `recXfade_wav` | `0x391342` | 48000 frames, 2 channels | periodic sine/cosine magnitude curves with approximately 1.7 exponent |

For `t = index / 47999`, numerical comparisons give:

- Playback channel 0 matches `(1 - cos(pi*t))/2`, and channel 1 matches its
  complement, with maximum absolute errors about `3.055e-5` (one PCM16 step).
  The stored channel sums range from `0.99993896484375` to
  `0.999969482421875`; they are not exactly one.
- Mix rising channel fits `sin(pi*t/2)^p` with a log-domain fitted
  `p = 1.3000883`, maximum error `3.052e-5`. This supports an intended exponent
  near 1.3; it is not proof of the exact authoring/quantization formula.
- Record channel 1 matches `sin(pi*t)^1.7`, maximum error `3.055e-5`.
  A subsequent full comparison of channel 0 against `abs(cos(pi*t))^1.7`
  also gives a maximum absolute error of `3.0546354e-5`.

These results identify likely analytic curves, **not bit-exact replacements**.
Exact reproduction additionally requires identifying the quantizer, table-index
rounding/interpolation, and the role of each curve at every call site.

## DGenLisp prerequisite: reproduced failure

The installed compiler SHA-256 is:
`168c0001a4a78c81bde94d214af18d17502dcd8701de4c28d3373fab83fd5d2e`.

From `crates/sequencer`, the following independent compile probes both fail
before code generation:

```sh
out=$(mktemp -d /tmp/granular-looper-probe.XXXXXX)
printf '(def b (tensor @shape [8]))\n(out (poke b 0 0 (in 1)) 1)\n' |
  tools/DGenLisp-macos-arm64 compile - -o "$out/poke" --name poke-probe
# Error: Unknown operator: poke

printf '(out (seq (in 1) (in 2)) 1)\n' |
  tools/DGenLisp-macos-arm64 compile - -o "$out/seq" --name seq-probe
# Error: Unknown operator: seq
```

The argument order in the first probe is only a proposed tensor-first API,
consistent with the underlying Swift API. An unknown-operator error occurs
before argument checking; changing argument order cannot fix it.

Local compiler checkout inspected: `~/code/swift/dgen`, HEAD
`6f6497090b2e440c6d0757d13f1e6f6c7bbb0eca`.

- `Sources/DGenLisp/LispEvaluator.swift` dispatches `peek` but neither `poke`
  nor `seq`. Its README nevertheless mentions recording with `poke`.
- `Sources/DGen/Tensor.swift` has a lower-level `poke(tensor:index:channel:value:)`
  that emits a memory write. Unlike `peek`, it directly accesses `shape[1]`;
  exposing it without rank validation/1D promotion would be unsafe.
- `Sources/DGen/Emit+State.swift` implements `.seq` by returning the last input
  and **assuming** inputs have already been emitted in the desired order.
  This alone does not establish an effect-ordering contract across scheduling,
  CSE, DCE, aliases, or block fusion. Those compiler passes were not fully audited.

A production-quality prerequisite must establish observable read/write ordering,
not merely add two evaluator cases. It needs exact render regressions for
write-before-read, read-before-write, repeated writes, aliased reads, persistent
state, 1D/2D destinations, wrapped indices, and block sizes 1/64/512. A returned
`seq` value must not allow the optimizer to hoist reads across writes. The
published compiler distribution and pin must then be updated normally.

## Earlier native candidate and reference comparison

A temporary candidate exists at `/tmp/granular-looper-dsp/dsp.lisp`. It is **not
installed** in the app's `.local/effects` library. Compilation alone is not a
release gate: the candidate still fails reference comparison and its temporary
48 kHz buffer allocations are unsuitable for deployment at arbitrary host rates.
`samplerate` is a runtime signal, not a constant accepted by `zeros` for sizing
storage. A deliberate capacity contract is required, not a sample-rate literal
silently presented as general support.

The isolated reference environment is `/tmp/granular-looper-reference-env`.
The comparison harness and table extractor are in
`/tmp/granular-looper-analysis.7esH2x/{compare,extract_tables}.py`.
These paths are temporary research artifacts, not installed effect dependencies.

New instruction-level mappings:

| Processor parameter pointer | Meaning | Core value |
| --- | --- | --- |
| `0x1298` | playback Hz | `0xee8` |
| `0x12a0` | playback offset (%) | `0xef8`, divided by 100 |
| `0x12a8` | play length (ms) | `0xeec` |
| `0x12b0` | crossfade length (%) | `0xef0`, divided by 100 |
| `0x12b8` / `0x12c0` | input / wet gain (dB) | `0xefc` / `0xf00`, `10^(dB/20)` |
| `0x12c8` / `0x12d0` | slew time / curve (%) | `0xf04` / `0xf14` |
| `0x12d8` | dry (%) | `0xf08`, `1 - dry/100` |
| `0x12e0` | feedback overdub (%) | `0xf0c`, square root of fraction |
| `0x12e8` | polyphony | `0xf10`, rounded integer |
| `0x12f0` / `0x12f8` | slew ×10 / Hz ×0.1 | time / rate multipliers |
| `0x1300` / `0x1308` | trigger / one-shot | `0xf20` / `0xf31` |
| `0x1310` / `0x1318` | skid / stream-lock | `0xf19` / `0xf18` |

- Total recording period targets `play_ms * sample_rate / 1000 * (1+x)`.
  The rounded, slewed period divided by `(1+slewed_x)` yields playback span;
  crossfade span is `slewed_x * playback_span`.
- Slew duration in milliseconds is
  `(0.001 + 74.999 * (time_percent/100)^1.5) * (10 if enabled else 1)`.
  Curve selection truncates `299 * curve_percent/100` to an integer channel.
  The 50-frame curve is linearly interpolated. Each transition retains its
  start and difference, then directionally clamps to its target; the zero-
  difference case matters during initialization.
- `prepareToPlay` passes `host_rate * 2^factor` to the core. The constructor
  initializes factor to zero, so the default reference path is native rate.
- `reset` sets most linear-ramp durations to 20 ms, the rate ramp to 66 ms,
  and head-weight smoothing to `1/(sample_rate*0.066)`. Stream-lock recording
  uses a 50 ms crossfade and stops the write cursor only when locked.
- ARM64 `fmsub(a,b,c)` means `c-a*b`, not `a*b-c`.
  Both channels of even-numbered heads subtract from their running sums.
  `fnmadd(a,b,c)` means `-(a*b+c)`: the boundary delay stores the **negative
  DC-blocked wet output**, not a different DC-filter recurrence.
- The DC pole is the float `0.9997000098228455`.
  Normalization is `(0.725 + 0.275*sqrt(linearly_smoothed_x)) * weight_sum^-0.4`.
- DGen scalar `peek`/`poke` storage is **planar**: `channel * samples + index`.
  Embedded WAV PCM is interleaved; the extractor must transpose to planar data.
  Skipping that transpose produces a compilable but completely incorrect effect.

Observed validation (not a passing equivalence claim):

- The reference loads and renders normally through Pedalboard 0.9.24.
- The native candidate compiles with the pinned v0.1.8 compiler and passes its
  inline binary audit. Its first 4096 samples are identical for blocks 1 and 512.
- With 0.5 s of silent rate-zero preparation, then rate one and an impulse train,
  the first reference wet output is at sample 24000, versus 25585 in the
  candidate. Maximum sample difference is `0.17776422`.
- The 1585-sample displacement is close to half the recovered 66 ms rate ramp
  at 48 kHz. This is a lead for investigating host parameter/preparation and
  phase-update behavior, **not permission to add a compensating offset**.
- Startup/dry-ramp behavior also differed in that earlier candidate. These
  observations predate the local listening version and its explicit snapshot
  ordering changes.

## Confidence boundary

The local effect is an auditionable reconstruction, not a bit-exact clone.
The initial mutable-buffer language blocker was resolved by the compiler release
described above. The user's larger-buffer proposal supplies an explicit bounded
capacity contract without requiring runtime tensor allocation.

Even after the compiler prerequisite, a complete reconstruction still requires
parameter mapping, write-head movement and overdub equations, trigger/one-shot
and stream-lock state transitions, slew-curve reconstruction, oversampling
configuration, reset semantics, and complete sample ordering. Static analysis
can continue independently of the compiler work.

The earlier black-box comparison failed, as recorded above. The listening
version has passed offline rendering checks but has not been requalified for
reference equivalence or interactively auditioned by the agent. Controlled
reference comparisons remain necessary only before claiming equivalence, not
before the user can listen to this explicitly experimental local effect.
Task state and dependencies live in Beads, not in this research document.
