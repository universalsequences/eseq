# Authored effect latency

Audio effects can declare their immutable processing delay in samples:

```lisp
(effect-latency 511)
```

For sample-rate-dependent algorithms, use a constant expression:

```lisp
(effect-latency (+ 31 (round (* samplerate 0.001))))
```

This example declares 75 samples at 44.1 kHz and 79 at 48 kHz. It does not add
any audio delay itself: the declaration must describe what the DSP actually
implements. Measure an impulse and inspect the algorithm; a buffer's window
size or hop size alone does **not** establish its end-to-end latency.

## Contract

- At most one declaration, at the top level of an audio-effect source.
- It is host metadata, not a parameter, signal, modulator, or p-lock target.
- Allowed values: numeric literals and `samplerate` (`sample-rate` alias).
- Operators: variadic `+`/`*` (one or more arguments), unary/binary `-`, binary
  `/`, `min`, `max`, `pow`, and unary `round`, `floor`, `ceil`, `log2`.
- No parameter references, arbitrary Lisp evaluation, or references to DSP
  `def` bindings. Repeat the relevant compile-time constants explicitly.
- Arithmetic uses finite float64 values. `round` rounds ties away from zero.
  The final result must be an integer; rounding is never silently inserted.
- The range is 0–16383 samples, bounded by the existing PDC delay-node capacity.
  Unknown operators, division by zero, fractional/negative/overflow results,
  duplicate/nested declarations and invalid metadata fail compilation.
- Resolution happens at the requested instance sample rate. Recompile/recreate
  an instance when changing sample rate; do not reuse an artifact at another rate.
- Omission preserves the existing zero-latency default and legacy builtin
  providers. Explicit zero overrides those providers.

Declare **transport delay**, not frequency-dependent phase/group delay or a
reverb tail. If controls change actual latency, keep the audio path padded to
a constant declared maximum, or wait for a future dynamic-latency contract.
The current declaration must remain correct across all audible settings.
An effect's internal Mix=0 path must retain its declared delay. Host bypass is
different: the host bypasses DSP without delay, so a bypassed slot contributes
zero and the PDC plan is recalculated.

## Host pipeline

`effect-latency` belongs to ESeq's authoring frontend, not the external DGenLisp
compiler. The saved source, patcher's host-declaration metadata, serialized graph payload,
source imports and cache key retain the declaration. Immediately before external compilation, the host
validates/resolves it and replaces only its byte span with whitespace, preserving
line and byte positions for compiler diagnostics. No hidden DSP memory cell is
created and no compiler binary update is required.

The host augments the compiler manifest with its own versioned namespace:

```json
{"eseqEffect":{"version":1,"latencySamples":79}}
```

Cached artifacts preserve and validate this metadata on reload. The resolved
value reaches `EffectDescriptor::declared_latency_samples` through track, bus,
and rack descriptor construction. Existing PDC sums running serial processors,
aligns supported parallel joins, and includes the mix delay in recording/export
accounting. Replacing an effect or bypassing it changes the topology's reported
delay; no effect-name registration is needed.

This does **not** remove existing planner limitations: aggregate alignment pads
can exceed the 16383-sample capacity (the live planner currently logs/clamps);
intra-rack alignment remains subject to the existing rack-PDC implementation;
live PDC delay changes still use the existing transition behavior. This work
neither infers nor retroactively declares the latency of other factory effects.
Those remain separate latency-epic work.

## External authoring tools

Do not pass an ESeq source declaration directly to the standalone DGenLisp
executable. Use the same host frontend as the app:

```sh
cargo build -p sequencer --bin dgen_effect_compile
target/debug/dgen_effect_compile /absolute/path/to/dsp.lisp --sample-rate 48000
```

The command compiles without opening the UI or audio device and prints the full
compiler/host manifest as JSON. `dylib` is an absolute path to the generated
scratch artifact. Asset references resolve relative to the source directory.
Callers own copying/retaining artifacts they need beyond scratch-directory
cleanup. This command does not audition audio or assert that authored latency
matches the DSP.

## Focused coverage

`lisp_host::dgen::effect_latency::tests` covers validation, sample-rate evaluation,
manifest persistence across cache-manager restarts, uncached compilation, and
patcher writeback. `app::graph::latency::tests::authored_latency_*` covers real
compiled serial delays aligned with a parallel dry branch, explicit zero,
bypass accounting, and replacement/bypass pad updates in a headless app.
No project-specific or private-effect fixture is required.

The builtin ES Compressor (`crates/sequencer/src/effects/es_compressor_dsp.lisp`)
is the first factory effect to use the declaration; bundled builtins compile
through the same path, so no name registration is needed for them either.
